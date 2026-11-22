#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Cold/warm adaptive FDW preparation and actual-selected-SQL direct controls."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time
import urllib.request
import uuid

from fdw_profile import redact


def foreign(plan):
    if plan['Node Type'] == 'Foreign Scan':
        return plan
    for child in plan.get('Plans', []):
        scan = foreign(child)
        if scan:
            return scan
    return None


def decode_plans(output):
    pid, remaining = output.split('\n', 1)
    decoder = json.JSONDecoder()
    plans = []
    while remaining.strip():
        value, end = decoder.raw_decode(remaining.lstrip())
        remaining = remaining.lstrip()[end:]
        plans.append(value[0])
    if len(plans) != 3:
        raise ValueError('expected cold, warm and manual plans from one backend')
    return int(pid), plans


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--pg-container', required=True)
    parser.add_argument('--direct-worker', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    out = Path(args.output)
    out.mkdir(parents=True, exist_ok=True)
    if (out / 'raw-adaptive.json').exists():
        raise ValueError('choose a fresh output directory')
    command = ['docker', 'exec', '-i', args.pg_container, 'timeout',
               '--signal=TERM', '--kill-after=5s', '120s', 'psql', '-U',
               'postgres', '-d', 'postgres', '-XAtq', '-v', 'ON_ERROR_STOP=1']

    def pg(sql):
        result = subprocess.run(command, input=sql, text=True,
                                capture_output=True, timeout=130)
        if result.returncode:
            raise RuntimeError(redact(result.stderr))
        return result.stdout.strip()

    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def ch(sql):
        request = urllib.request.Request(os.environ['ANTALYA_TEST_URL'],
            data=sql.encode(), headers={'Authorization': 'Bearer ' +
                                       os.environ['ANTALYA_TEST_TOKEN']})
        with opener.open(request, timeout=60) as response:
            return response.read().decode()

    prefix = 'fdw_adaptive_' + uuid.uuid4().hex
    automatic, manual = prefix + '_auto', prefix + '_manual'
    result = {'settings': {
        'postgres': pg('SELECT version()'), 'engine': ch('SELECT version()').strip(),
        'terminal_policy': 'flush_first', 'batch_bytes': 16777216,
        'expected_rows': 100, 'timing': 'OFF',
        'order': ['auto_cold', 'auto_warm', 'manual_exact'],
        'cache_boundary': 'Three scans in one fresh backend; cold means backend adaptive cache only, not server or OS caches.',
        'direct_boundary': 'Fresh published client per actual analyzed selected SQL; fetch and whole-process clocks retained separately.',
        'clock_contract': 'Preparation and nested transfer/decode/terminal clocks overlap; no exclusive subtraction.'},
        'postgres': [], 'direct': []}
    pending = out / 'pending-adaptive.json'

    def preserve():
        pending.write_text(json.dumps(result, indent=2) + '\n')

    try:
        pg("ALTER SERVER fdw_profile OPTIONS (SET terminal_policy 'flush_first', SET batch_bytes '16777216'); "
           f"CREATE FOREIGN TABLE {automatic}(id bigint,url text,extra text) SERVER fdw_profile OPTIONS(table 'fdw_profile.events', candidate_columns 'url', candidate_mode 'auto'); "
           f"CREATE FOREIGN TABLE {manual}(id bigint,url text,extra text) SERVER fdw_profile OPTIONS(table 'fdw_profile.events');")
        statements = ['SELECT pg_backend_pid()']
        for table in [automatic, automatic, manual]:
            statements.append(f"EXPLAIN(ANALYZE,VERBOSE,TIMING OFF,FORMAT JSON) SELECT id,url FROM {table} WHERE wc_match(url,'*powershell*','raw')")
        start = time.monotonic()
        output = pg('; '.join(statements) + ';')
        result['postgres_session_process_wall_ms'] = (time.monotonic() - start) * 1000
        # Preserve raw successful session output before interpretation/controls.
        result['postgres_session_output'] = output
        preserve()
        pid, plans = decode_plans(output)
        for label, plan in zip(result['settings']['order'], plans):
            scan = foreign(plan['Plan'])
            record = {'case': label, 'backend_pid': pid, 'plan': plan,
                      'query_id': scan['FDW last query ID'],
                      'remote_sql': scan['ClickHouse SQL'],
                      'wall_ms': plan['Execution Time']}
            result['postgres'].append(record)
            preserve()
            if (scan['Actual Rows'] != 100 or scan['FDW remote rows'] != 100
                    or not scan['FDW metrics complete']
                    or scan['FDW row timing enabled']
                    or scan.get('Rows Removed by Filter', 0)):
                raise RuntimeError('adaptive PostgreSQL result oracle failed')
        cold, warm, exact = [foreign(record['plan']['Plan'])
                             for record in result['postgres']]
        result['cache_oracle'] = {
            'cold_probes_observed': cold['FDW plan probes'] > 0,
            'warm_cache_hit_observed': warm['FDW plan cache hits'] > 0,
            'warm_did_not_probe': warm['FDW plan probes'] == 0,
            'manual_did_not_prepare': exact['FDW plan probes'] == 0
                                      and exact['FDW plan cache hits'] == 0}
        preserve()
        if not all(result['cache_oracle'].values()):
            raise RuntimeError('adaptive cold/warm cache oracle failed')
        for record in result['postgres']:
            start = time.monotonic()
            worker = subprocess.run([args.direct_worker], text=True,
                input=json.dumps({'sql': record['remote_sql'], 'policy': 'flush_first',
                                  'batch_bytes': 16777216}),
                capture_output=True, timeout=310)
            elapsed = (time.monotonic() - start) * 1000
            if worker.returncode:
                raise RuntimeError(redact(worker.stderr))
            direct = json.loads(redact(worker.stdout))
            direct.update(case=record['case'], remote_sql=record['remote_sql'],
                          process_wall_ms=elapsed)
            result['direct'].append(direct)
            preserve()
            if direct['rows'] != 100:
                raise RuntimeError('adaptive direct result oracle failed')
        ch('SYSTEM FLUSH LOGS')
        for record in result['postgres'] + result['direct']:
            qid = record['query_id']
            if not re.fullmatch(r'[A-Za-z0-9-]+', qid):
                raise ValueError('unexpected query ID spelling')
            records = json.loads(ch("SELECT query_id,query_duration_ms,read_rows,read_bytes,result_rows,result_bytes,memory_usage,ProfileEvents FROM system.query_log WHERE type='QueryFinish' AND query_id='" + qid + "' FORMAT JSON"))['data']
            record['query_log'] = records
            preserve()
            if len(records) != 1 or int(records[0]['result_rows']) != 100:
                raise RuntimeError('adaptive native correlation failed')
    except Exception as error:
        result['error'] = redact(str(error))
    finally:
        try:
            pg(f'DROP FOREIGN TABLE IF EXISTS {automatic}, {manual}')
        except Exception as error:
            result['cleanup_error'] = redact(str(error))
        (out / 'raw-adaptive.json').write_text(json.dumps(result, indent=2) + '\n')
        pending.unlink(missing_ok=True)
    if 'error' in result or 'cleanup_error' in result:
        raise RuntimeError('adaptive supplement failed; sanitized evidence retained')
    summary = []
    for record, direct in zip(result['postgres'], result['direct']):
        scan = foreign(record['plan']['Plan'])
        summary.append({'case': record['case'], 'backend_pid': record['backend_pid'],
            'postgres_ms': record['wall_ms'], 'direct_fetch_ms': direct['wall_ms'],
            'direct_process_ms': direct['process_wall_ms'],
            'preparation': {k: v for k, v in scan.items()
                            if k.startswith('FDW plan') or k.startswith('FDW automatic')},
            'remote_sql': record['remote_sql']})
    (out / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary), flush=True)


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        raise SystemExit(redact(str(error))) from None
