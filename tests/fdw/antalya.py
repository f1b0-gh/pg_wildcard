#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Actual bearer/Arrow FDW qualification; run under antalya-rs/scripts/fixture.py.

Requires the extension installed in PGBIN's PostgreSQL and psycopg is not needed.
Creates its own PostgreSQL cluster and Antalya database and removes both.
"""
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.request

URL = os.environ['ANTALYA_TEST_URL']
TOKEN = os.environ['ANTALYA_TEST_TOKEN']
PGBIN = Path(os.environ['PGBIN']) if os.environ.get('PGBIN') else Path(
    subprocess.check_output(['pg_config', '--bindir'], text=True).strip()
)
# A fresh private directory keeps Unix socket paths short and avoids depending
# on the retired shared workspace.
work = Path(tempfile.mkdtemp(prefix='wc-fdw-antalya-', dir='/tmp'))
database = 'fdw_' + secrets.token_hex(5)
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0)); port = str(sock.getsockname()[1])

def command(*args):
    return subprocess.run(args, capture_output=True, text=True, check=True).stdout.strip()

def pg(sql, *, ok=True):
    result = subprocess.run([str(PGBIN / 'psql'), '-h', str(work), '-p', port, '-d', 'postgres', '-XAtq', '-v', 'ON_ERROR_STOP=1'], input=sql, capture_output=True, text=True)
    if ok and result.returncode:
        # Never include SQL, which may contain a mapping credential.
        raise RuntimeError('PostgreSQL failed: ' + result.stderr)
    if not ok:
        assert result.returncode and TOKEN not in result.stderr, 'expected redacted PostgreSQL error'
        return result.stderr
    return result.stdout.strip()

def ch(sql):
    request = urllib.request.Request(URL, data=sql.encode(), headers={'Authorization': 'Bearer ' + TOKEN})
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=30) as response:
        return response.read().decode().strip()

def check(condition, label):
    assert condition, label
    print('ok  ' + label, flush=True)

def foreign(name, columns, expression):
    pg(f"CREATE FOREIGN TABLE {name} ({columns}) SERVER ch OPTIONS (table '{expression.replace(chr(39), chr(39)*2)}')")

started = False
try:
    command(str(PGBIN / 'initdb'), '-D', str(work / 'pg'), '-A', 'trust', '-U', os.environ['USER'])
    command(str(PGBIN / 'pg_ctl'), '-D', str(work / 'pg'), '-l', str(work / 'postgres.log'), '-o', f"-p {port} -k {work} -c listen_addresses=''", '-w', 'start')
    started = True
    pg('CREATE EXTENSION pg_wildcard')
    ch(f'CREATE DATABASE {database}')
    ch(f"CREATE TABLE {database}.events ENGINE=Memory AS SELECT toInt64(number) id, concat('https://', if(number%3=0,'evil','good'), '.example/', toString(number)) url FROM numbers(1000)")
    pg(f"CREATE SERVER ch FOREIGN DATA WRAPPER wildcard_fdw OPTIONS (url '{URL}', database '{database}', timeout_ms '10000')")
    pg(f"CREATE USER MAPPING FOR CURRENT_USER SERVER ch OPTIONS (bearer_token '{TOKEN}')")
    foreign('events', 'id bigint, url text', f'{database}.events')
    foreign('identity', 'principal text', '(SELECT currentUser() principal)')
    check(pg('SELECT principal FROM identity') == 'fixture', 'bearer executes as the expected fixture principal')
    check(pg('SELECT count(*),sum(id) FROM events') == '1000|499500', 'ordinary Arrow scan and terminal completion')
    pg('CREATE TABLE local_events AS SELECT * FROM events')
    for predicate in ["wc_match(url, '*evil.example/*')", "wc_like(url, '%/1%')", "wc_regex(url, 'evil') AND id>=33", "url LIKE '%good%' AND id<500", 'id IS NOT NULL']:
        query = f"SELECT coalesce(string_agg(id::text, ',' ORDER BY id),'') FROM {{table}} WHERE {predicate}"
        check(pg(query.format(table='events')) == pg(query.format(table='local_events')), 'local/remote exact parity: ' + predicate)
        plan = pg(f'EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF, SUMMARY OFF) SELECT * FROM events WHERE {predicate}')
        check(' WHERE ' in plan and 'Rows Removed by Filter' not in plan, 'pushdown exact with local recheck retained')
    result = pg('SELECT sum((SELECT count(*) FROM events WHERE id >= g)) FROM generate_series(0,2) g')
    check(result == '2997', 'correlated rescans retain local Param quals')
    plan = pg('EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF, SUMMARY OFF) SELECT sum((SELECT count(*) FROM events WHERE id >= g)) FROM generate_series(0,2) g')
    check('loops=3' in plan and 'Foreign Scan' in plan, 'PostgreSQL executes actual rescan callbacks')
    foreign('types', 'b bytea, n text, s text, flag boolean, small smallint, f real, d date, ts timestamptz', "(SELECT unhex('6100ff') b, CAST(NULL AS Nullable(String)) n, 'a\\tb\\nc\\\\d' s, true flag, toInt16(-32768) small, toFloat32(1.25) f, toDate('2024-02-29') d, toDateTime64('2024-02-29 12:34:56.123456',6,'UTC') ts)")
    check(pg("SET timezone='Asia/Tokyo'; SELECT encode(b,'hex'),n IS NULL,encode(convert_to(s,'UTF8'),'hex'),flag,small,f,d,(ts AT TIME ZONE 'UTC')::text FROM types") == '6100ff|t|6109620a635c64|t|-32768|1.25|2024-02-29|2024-02-29 12:34:56.123456', 'NULL, binary, escapes, bool, integers, floats, date and UTC microseconds')
    for name, column, expression, error in [
        ('bad_text', 's text', "(SELECT unhex('ff') s)", 'invalid UTF-8'),
        ('nul_text', 's text', "(SELECT unhex('610062') s)", 'NUL'),
        ('overflow', 'n bigint', '(SELECT toUInt64(18446744073709551615) n)', 'overflows'),
        ('ns', 'ts timestamptz', "(SELECT fromUnixTimestamp64Nano(1,'UTC') ts)", 'precision'),
        ('unsupported', 'n numeric', '(SELECT toDecimal64(1,2) n)', 'unsupported PostgreSQL'),
        ('modifier', 's varchar(1)', "(SELECT 'long' s)", 'type modifier'),
        ('mismatch', 'n text', '(SELECT toUInt64(1) n)', 'type pairing'),
        ('oversize', 's text', "(SELECT concat(repeat('a',524288),repeat('b',524288),'c') s)", '1 MiB'),
        ('zone', 'ts timestamptz', "(SELECT toDateTime64('2024-01-01',6,'Europe/Paris') ts)", 'type pairing'),
    ]:
        foreign(name, column, expression)
        check(error in pg(f'SELECT * FROM {name}', ok=False), 'explicit refusal: ' + name)
    foreign('empty_valid', 'n bigint', '(SELECT toInt64(number) n FROM numbers(0))')
    check(pg('SELECT count(*) FROM empty_valid') == '0', 'valid empty Arrow result retains its schema and confirmed completion')
    foreign('empty_mismatch', 'n text', '(SELECT toUInt64(number) n FROM numbers(0))')
    check('type pairing' in pg('SELECT * FROM empty_mismatch', ok=False), 'empty unsupported Arrow type is refused')
    foreign('empty_missing', 'absent bigint', '(SELECT toInt64(number) n FROM numbers(0))')
    check('wildcard_fdw:' in pg('SELECT * FROM empty_missing', ok=False), 'empty result cannot hide a missing projected column')
    foreign('late_error', 'id bigint, bad boolean', '(SELECT toInt64(number) id, throwIf(number>=2048) bad FROM numbers(10000))')
    check('wildcard_fdw:' in pg('SELECT count(bad) FROM late_error', ok=False), 'server exception cannot become successful EOF')
    partial = subprocess.run([str(PGBIN / 'psql'), '-h', str(work), '-p', port, '-d', 'postgres', '-XAtq', '-v', 'ON_ERROR_STOP=1', '-c', 'BEGIN; DECLARE c CURSOR FOR SELECT * FROM late_error; FETCH 1 FROM c; FETCH ALL FROM c; COMMIT'], capture_output=True, text=True)
    check(partial.returncode != 0 and partial.stdout.startswith('0|f') and 'wildcard_fdw:' in partial.stderr, 'late engine failure remains visible after a provisional PostgreSQL cursor row')
    foreign('early', 'number bigint', 'numbers(10000)')
    check(pg('SELECT number FROM early LIMIT 1') == '0', 'early stop returns its row and drains to confirmed completion')
    held = pg('BEGIN; DECLARE held_a CURSOR FOR SELECT number FROM early; FETCH 1 FROM held_a; DECLARE held_b CURSOR FOR SELECT number FROM early; FETCH 1 FROM held_b; DECLARE held_c CURSOR FOR SELECT number FROM early; FETCH 1 FROM held_c; SELECT sum((SELECT number FROM early WHERE number>=g LIMIT 1)) FROM generate_series(0,2) g; CLOSE held_a; CLOSE held_b; CLOSE held_c; COMMIT')
    check(held.splitlines() == ['0','0','0','3'], 'active rescans with three held cursors confirm old scan before replacement')
    recovered = pg('BEGIN; DECLARE early_cursor CURSOR FOR SELECT number FROM early; FETCH 1 FROM early_cursor; CLOSE early_cursor; SELECT count(*) FROM events; COMMIT')
    check(recovered.splitlines() == ['0','1000'], 'same backend remains usable after confirming an early-stop cursor')
    foreign('slow', 'id bigint, s boolean', '(SELECT toInt64(number) id, sleepEachRow(0.001) s FROM numbers(1000))')
    before_cancel = time.monotonic()
    err = pg("SET statement_timeout='100ms'; SELECT count(s) FROM slow", ok=False)
    check('statement timeout' in err and time.monotonic() - before_cancel < 3, 'PostgreSQL interrupts remain responsive while worker awaits HTTP')
    pg("ALTER SERVER ch OPTIONS (SET url 'http://127.0.0.1:1')")
    check('Foreign Scan' in pg('EXPLAIN SELECT * FROM events'), 'EXPLAIN performs no network I/O')
    pg(f"ALTER SERVER ch OPTIONS (SET url '{URL}')")
    pg("ALTER USER MAPPING FOR CURRENT_USER SERVER ch OPTIONS (ADD password 'secret-sentinel')")
    err = pg('SELECT * FROM events', ok=False)
    check('legacy user/password' in err and 'secret-sentinel' not in err, 'legacy auth refused with redacted migration message')
    pg('ALTER USER MAPPING FOR CURRENT_USER SERVER ch OPTIONS (DROP password)')
    rotated = subprocess.run([str(PGBIN / 'psql'), '-h', str(work), '-p', port, '-d', 'postgres', '-XAtq', '-v', 'ON_ERROR_STOP=1'], input="SELECT count(*) FROM events; ALTER USER MAPPING FOR CURRENT_USER SERVER ch OPTIONS (SET bearer_token 'invalid-sentinel-token'); SELECT count(*) FROM events;", capture_output=True, text=True)
    check(rotated.returncode != 0 and rotated.stdout.strip() == '1000' and 'wildcard_fdw:' in rotated.stderr and 'invalid-sentinel-token' not in rotated.stderr, 'same-backend bearer rotation cannot reuse the prior authorized client')
    pg('ALTER USER MAPPING FOR CURRENT_USER SERVER ch OPTIONS (DROP bearer_token)')
    check('bearer_token is required' in pg('SELECT * FROM events', ok=False), 'missing bearer mapping is refused without anonymous fallback')
    pg(f"ALTER USER MAPPING FOR CURRENT_USER SERVER ch OPTIONS (ADD bearer_token '{TOKEN}')")
    # Same backend, same cached client. Query-log records source ports for HTTP scans.
    pg('SELECT count(id),count(url) FROM events; SELECT count(id),count(url) FROM events; SELECT count(id),count(url) FROM events')
    ch('SYSTEM FLUSH LOGS')
    ports = ch(f"SELECT count(),countDistinct(port),min(port)>0 FROM (SELECT port FROM system.query_log WHERE current_database='{database}' AND startsWith(query_id,'wildcard-') AND type='QueryStart' AND query LIKE 'SELECT `id`, `url` FROM%' ORDER BY event_time_microseconds DESC LIMIT 3)")
    check(ports == '3\t1\t1', 'three scans reuse one persistent Arrow HTTP connection')
    print('PASS actual pinned bearer Antalya FDW qualification; logs: ' + str(work), flush=True)
finally:
    if started:
        subprocess.run([str(PGBIN / 'pg_ctl'), '-D', str(work / 'pg'), '-m', 'immediate', '-w', 'stop'], capture_output=True)
    try:
        ch(f'DROP DATABASE IF EXISTS {database}')
    except Exception:
        pass
    # Logs contain no SQL/token (psql statements are never printed).
    shutil.rmtree(work / 'pg', ignore_errors=True)
