#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""PG18.6 + native Antalya optimization qualification, inside owned fixtures.

Run via antalya-rs/scripts/fixture.py after installing the extension in an owned
PG container. PG_OPT_CONTAINER and PG_OPT_SOCKET identify that container/cluster.
No credentials are logged or passed as command-line arguments.
"""
import json
import os
import re
import subprocess
import urllib.request

container = os.environ['PG_OPT_CONTAINER']
socket = os.environ['PG_OPT_SOCKET']
port = int(os.environ.get('PG_OPT_PORT', '5432'))
assert 1 <= port <= 65535
url, token = os.environ['ANTALYA_TEST_URL'], os.environ['ANTALYA_TEST_TOKEN']
native = os.environ['ANTALYA_TEST_CONTAINER']
assert re.fullmatch(r'(?:libwildcard-(?:opt|stream)-pg|wildcard-engine-next-pg)-[a-z0-9]+', container)
assert re.fullmatch(r'antalya-rs-[a-f0-9]{12}', native)
address = subprocess.check_output(['docker', 'inspect', '-f', '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}', native], text=True).strip()
assert all(c in '0123456789.' for c in address)


def pg(sql, failure=False):
    p = subprocess.run(['docker','exec','-i',container,'psql','-h',socket,'-p',str(port),'-U','postgres','-d','postgres','-XAtq','-v','ON_ERROR_STOP=1'],input=sql,text=True,capture_output=True)
    if failure:
        assert p.returncode and token not in p.stderr
        return p.stderr
    if p.returncode:
        raise RuntimeError('PostgreSQL qualification error: '+p.stderr.replace(token,'[redacted]'))
    return p.stdout.strip()


def ch(sql):
    req = urllib.request.Request(url, data=sql.encode(), headers={'Authorization':'Bearer '+token})
    with urllib.request.urlopen(req, timeout=30) as r:
        return r.read().decode().strip()


def scan(sql, analyze=False, timing=True):
    plan=json.loads(pg('EXPLAIN ('+(('ANALYZE, ' if timing else 'ANALYZE, TIMING OFF, ') if analyze else '')+'FORMAT JSON) '+sql))[0]['Plan']
    def find(p):
        if p['Node Type']=='Foreign Scan':return p
        for child in p.get('Plans',[]):
            found=find(child)
            if found:return found
    return find(plan)


def check(condition, label):
    assert condition, label
    print('ok  '+label,flush=True)


def early_stop_failure():
    """Real PostgreSQL executor, simulated late server failure after valid Arrow."""
    from pathlib import Path
    fixture=subprocess.Popen(['docker','exec','-i',container,'python3',str(Path(__file__).resolve().with_name('late_failure.py'))],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
    fixture.stdin.write(json.dumps({'token':token})+'\n');fixture.stdin.flush()
    port=int(fixture.stdout.readline())
    def observed():
        fixture.stdin.write('observations\n');fixture.stdin.flush()
        return int(fixture.stdout.readline())
    try:
        pg(f"CREATE SERVER opt_error FOREIGN DATA WRAPPER wildcard_fdw OPTIONS (url 'http://127.0.0.1:{port}',timeout_ms '3000')")
        pg(f"CREATE USER MAPPING FOR CURRENT_USER SERVER opt_error OPTIONS (bearer_token '{token}')")
        pg("CREATE FOREIGN TABLE opt_bad (n bigint) SERVER opt_error OPTIONS (table 'late_failure')")
        error=pg('SELECT n FROM opt_bad WHERE n=1 ORDER BY n LIMIT 1',failure=True)
        check('terminal evidence proves query failure' in error,'early LIMIT reports the terminal failure itself')
        check(observed()>0,'early LIMIT waits for terminal failure after provisional Arrow')
        previous=observed()
        error=pg('SELECT v.x,f.n FROM (VALUES(1),(2)) v(x) CROSS JOIN LATERAL (SELECT n FROM opt_bad WHERE n%v.x=0 LIMIT 1) f',failure=True)
        check('terminal evidence proves query failure' in error,'rescan reports the terminal failure itself')
        check(observed()>previous,'rescan/normal end cannot hide terminal failure')
    finally:
        pg('DROP SERVER IF EXISTS opt_error CASCADE')
        fixture.stdin.write('stop\n');fixture.stdin.flush()
        fixture.communicate(timeout=5)


ch('CREATE DATABASE opt_fdw')
try:
    ch("CREATE TABLE opt_fdw.events (id Int64, v String, extra String, INDEX v_ng v TYPE text(tokenizer='ngrams')) ENGINE=MergeTree ORDER BY id")
    ch("INSERT INTO opt_fdw.events SELECT toInt64(number), if(number%3=0,'powershell.exe','normal'), repeat('x',1024) FROM numbers(1000)")
    pg('CREATE EXTENSION IF NOT EXISTS pg_wildcard')
    pg(f"CREATE SERVER opt_native FOREIGN DATA WRAPPER wildcard_fdw OPTIONS (url 'http://{address}:8123', database 'opt_fdw', timeout_ms '10000')")
    pg(f"CREATE USER MAPPING FOR CURRENT_USER SERVER opt_native OPTIONS (bearer_token '{token}')")
    pg("CREATE FOREIGN TABLE opt_events (id bigint, v text, extra text) SERVER opt_native OPTIONS (table 'opt_fdw.events')")
    pg("CREATE FOREIGN TABLE opt_never (id bigint) SERVER opt_native OPTIONS (table 'opt_fdw.no_such_table')")
    check(pg('SELECT id FROM opt_never LIMIT 0')=='','LIMIT0 does not dispatch an unused remote scan')
    unused=scan('SELECT id FROM opt_never LIMIT 0',True)
    check(unused.get('FDW row timing enabled') is False and unused['FDW row conversion']==0 and unused['FDW remote rows']==0 and unused['Actual Loops']==0,'unused scan does not start conversion timing or dispatch')
    pg('CREATE TABLE opt_local AS SELECT * FROM opt_events')
    # Visibility is checked before applying the user's page bound, including
    # RLS on a local membership relation and a security-barrier invoker view.
    pg('CREATE ROLE opt_visible')
    pg('CREATE SCHEMA opt_security')
    pg('CREATE TABLE opt_security.membership(id bigint)')
    pg('INSERT INTO opt_security.membership VALUES (2),(5)')
    pg('ALTER TABLE opt_security.membership ENABLE ROW LEVEL SECURITY')
    pg('CREATE POLICY visible ON opt_security.membership FOR SELECT TO opt_visible USING (id=5)')
    pg('CREATE VIEW opt_security.events WITH (security_barrier=true,security_invoker=true) AS SELECT e.* FROM public.opt_events e WHERE EXISTS (SELECT 1 FROM opt_security.membership m WHERE m.id=e.id)')
    pg('GRANT USAGE ON SCHEMA public,opt_security TO opt_visible')
    pg('GRANT SELECT ON opt_events,opt_security.events,opt_security.membership TO opt_visible')
    pg(f"CREATE USER MAPPING FOR opt_visible SERVER opt_native OPTIONS (bearer_token '{token}')")
    visibility="SELECT id FROM opt_security.events WHERE wc_match(v,'*') ORDER BY id LIMIT 1"
    check(pg('SET ROLE opt_visible; '+visibility)=='5','RLS membership and security-barrier visibility precede top-K')
    visible_plan=pg('SET ROLE opt_visible; EXPLAIN(FORMAT JSON) '+visibility)
    check(' LIMIT ' not in visible_plan,'visibility-dependent page does not limit native scan early')
    pg('CREATE SCHEMA opt_collation')
    pg("CREATE COLLATION opt_collation.nocase (provider=icu, locale='und-u-ks-level2', deterministic=false)")
    pg("CREATE COLLATION opt_collation.locale_order (provider=icu, locale='en-US', deterministic=true)")
    ch('CREATE TABLE opt_fdw.collation (id Int64,v String) ENGINE=MergeTree ORDER BY id')
    ch("INSERT INTO opt_fdw.collation VALUES (1,'EVIL'),(2,'evil'),(3,'a'),(4,'Z'),(5,'é'),(6,'')")
    for name, collation in [('nocase','nocase'),('ordered','locale_order')]:
        pg(f"CREATE FOREIGN TABLE opt_collation.{name}(id bigint,v text COLLATE opt_collation.{collation}) SERVER opt_native OPTIONS (table 'opt_fdw.collation',candidate_columns 'v',candidate_prewhere 'true')")
        pg(f'CREATE TABLE opt_collation.{name}_local AS SELECT * FROM opt_collation.{name}')
    for name, predicate in [('nocase',"v LIKE '%evil%'"),('nocase',"v NOT LIKE '%evil%'"),('nocase',"v='evil'"),('nocase',"v<>'evil'"),('nocase',"v=('evil'::name COLLATE opt_collation.nocase)"),('ordered',"v<'Z'"),('ordered',"v>='Z'"),('ordered',"v<('Z'::name COLLATE opt_collation.locale_order)")]:
        query=f'SELECT id FROM opt_collation.{{t}} WHERE {predicate} ORDER BY id LIMIT 2'
        check(pg(query.format(t=name))==pg(query.format(t=name+'_local')),'locale predicate preserves all matching rows: '+predicate)
        plan=scan(query.format(t=name),True)
        check('WHERE' not in plan['ClickHouse SQL'] and ' LIMIT ' not in plan['ClickHouse SQL'],'unsafe collation stays local without candidate narrowing')
    query="SELECT id FROM opt_collation.nocase WHERE wc_match(v,'*evil*') ORDER BY id LIMIT 1"
    check(pg(query)=='2','wc_match retains its own case-sensitive semantics under ICU')
    check(' LIMIT 1' in scan(query)['ClickHouse SQL'],'extension semantics safely preserve ICU table top-K')
    query="SELECT id FROM opt_collation.nocase WHERE wc_match(v,'?','raw') ORDER BY id LIMIT 1 OFFSET 2"
    check(pg(query)=='5','Unicode single-character match and OFFSET retain exact page')
    unicode_plan=scan(query,True)
    check(unicode_plan['ClickHouse SQL'].endswith(' LIMIT 3') and unicode_plan.get('Rows Removed by Filter',0)==0,'Unicode top-K keeps local recheck without losing matches')
    queries=[
        'SELECT id,v FROM {t} ORDER BY id LIMIT 32',
        'SELECT id,v FROM {t} ORDER BY id DESC NULLS FIRST LIMIT 17 OFFSET 9',
        "SELECT id FROM {t} WHERE wc_match(v,'*powershell*','raw') ORDER BY id LIMIT 7",
        'SELECT id FROM {t} WHERE id=3 ORDER BY id LIMIT 1',
    ]
    plans=[]
    for query in queries:
        actual=query.format(t='opt_events'); expected=query.format(t='opt_local')
        check(pg(actual)==pg(expected),'ordered page matches local: '+query)
        p=scan(actual,True);plans.append(p)
        check(' ORDER BY ' in p['ClickHouse SQL'] and ' LIMIT ' in p['ClickHouse SQL'],'remote ordered limit present')
        check('`extra`' not in p['ClickHouse SQL'],'unused large column not transferred')
        check(p.get('Rows Removed by Filter',0)==0,'no local recheck removals')
    for query in [
        'SELECT id FROM {t} WHERE id%7=0 ORDER BY id LIMIT 5',
        'SELECT count(*) FROM {t}',
        'SELECT DISTINCT id FROM {t} ORDER BY id LIMIT 3',
        'SELECT id FROM {t} ORDER BY v LIMIT 3',
        'SELECT id FROM {t} ORDER BY id FETCH FIRST 2 ROWS WITH TIES',
    ]:
        q=query.format(t='opt_events');p=scan(q)
        check(' LIMIT ' not in p['ClickHouse SQL'],'unsupported upper/filter shape scans without remote limit')
        # Text ties do not define stable IDs; compare only defined-order queries.
        if 'ORDER BY v' not in query:check(pg(q)==pg(query.format(t='opt_local')),'fallback remains exact')
    p=scan('SELECT count(*) FROM opt_events')
    check('toUInt8(1) AS __wildcard_row' in p['ClickHouse SQL'],'count uses dummy projection, no aggregate pushdown')
    check('`v`' in scan("SELECT id FROM opt_events WHERE length(v)>5")['ClickHouse SQL'],'local-only qual column is retained')
    check('`extra`' in scan('SELECT opt_events FROM opt_events')['ClickHouse SQL'],'whole-row projection retains every attribute')
    pg("CREATE FOREIGN TABLE opt_unused (id bigint, unsupported jsonb) SERVER opt_native OPTIONS (table 'opt_fdw.events')")
    check(pg('SELECT count(id) FROM opt_unused')=='1000','unused unsupported column is not decoded')
    pg("CREATE FOREIGN TABLE opt_dropped (id bigint, discarded text, v text) SERVER opt_native OPTIONS (table 'opt_fdw.events')")
    pg('ALTER FOREIGN TABLE opt_dropped DROP COLUMN discarded')
    check(pg('SELECT id,v FROM opt_dropped ORDER BY id LIMIT 3')==pg('SELECT id,v FROM opt_local ORDER BY id LIMIT 3'),'projection preserves dropped physical positions')
    pg('CREATE SCHEMA opt_custom')
    pg('CREATE OPERATOR opt_custom.= (LEFTARG=bigint, RIGHTARG=bigint, FUNCTION=pg_catalog.int8ne)')
    q='SELECT id FROM opt_events WHERE id OPERATOR(opt_custom.=) 3::bigint ORDER BY id LIMIT 5'
    check(' LIMIT ' not in scan(q)['ClickHouse SQL'] and ' WHERE ' not in scan(q)['ClickHouse SQL'],'custom operator sharing builtin function is not pushed by name')
    check(pg(q)==pg(q.replace('opt_events','opt_local')),'custom operator retains local semantics')
    pg("CREATE FUNCTION opt_custom.wc_match(text,text,text) RETURNS boolean IMMUTABLE LANGUAGE SQL AS 'SELECT true'")
    # Inlining is legal; a true function must not become remote LIKE via its name.
    check(pg("SELECT count(*) FROM opt_events WHERE opt_custom.wc_match(v,'never-present','raw')")=='1000','same-name function does not become extension predicate')
    pg('DROP SCHEMA opt_custom CASCADE')
    ch("CREATE TABLE opt_fdw.nulls (id Int64,n Nullable(Int64)) ENGINE=Memory")
    ch("INSERT INTO opt_fdw.nulls VALUES (1,NULL),(2,5),(3,-1),(4,NULL),(5,5),(6,2)")
    pg("CREATE FOREIGN TABLE opt_nulls(id bigint,n bigint) SERVER opt_native OPTIONS (table 'opt_fdw.nulls')")
    pg('CREATE TABLE opt_null_local AS SELECT * FROM opt_nulls')
    for order in ['n ASC NULLS FIRST,id','n DESC NULLS LAST,id DESC']:
        q=f'SELECT id,n FROM opt_nulls ORDER BY {order} LIMIT 4 OFFSET 1'
        check(pg(q)==pg(q.replace('opt_nulls','opt_null_local')),'nullable integer ordering and offset are exact')
        check(' LIMIT 5' in scan(q)['ClickHouse SQL'],'offset adds to remote limit without remote offset')
    q='SELECT v.x,e.id FROM (VALUES(2),(3)) v(x) CROSS JOIN LATERAL (SELECT id FROM opt_events WHERE id%v.x=0 LIMIT 1) e'
    check(pg(q)==pg(q.replace('opt_events','opt_local')),'correlated residuals retain local semantics through rescans')
    p=scan(q,True)
    check(' LIMIT ' not in p['ClickHouse SQL'] and p['Actual Loops']==2,'join does not push an outer limit into its scan')
    check(p['FDW remote rows']==2000,'transport counters aggregate both remote scans')
    check(p['FDW metrics complete'] is False,'EXPLAIN labels early-stop counters incomplete before final drain')
    check(p.get('FDW row timing enabled') is True,'executor timing stays enabled through rescans')
    untimed_rescan=scan(q,True,timing=False)
    check(untimed_rescan['Actual Loops']==2 and untimed_rescan['FDW remote rows']==2000 and untimed_rescan.get('FDW row timing enabled') is False and untimed_rescan['FDW row conversion']==0 and untimed_rescan['FDW metrics complete'] is False,'TIMING OFF remains disabled across rescans without losing rows')
    pg("CREATE FOREIGN TABLE opt_child () INHERITS (opt_events) SERVER opt_native OPTIONS (table 'opt_fdw.events')")
    check(' LIMIT ' not in scan('SELECT id FROM opt_events ORDER BY id LIMIT 3')['ClickHouse SQL'],'inheritance does not push parent limit into child scans')
    check(pg('SELECT count(*) FROM opt_events')=='2000','inheritance retains parent and child rows')
    pg('DROP FOREIGN TABLE opt_child')
    early_stop_failure()
    pg("ALTER FOREIGN TABLE opt_events OPTIONS (ADD candidate_columns 'v', ADD candidate_prewhere 'true')")
    q="SELECT id FROM opt_events WHERE wc_regex(v,'powershell[.]exe',false,'raw') ORDER BY id LIMIT 7"
    check(' PREWHERE ' in scan(q)['ClickHouse SQL'],'candidate filter is staged before exact regex')
    check(pg(q)==pg(q.replace('opt_events','opt_local')),'prefilter plus exact regex matches local')
    pg("ALTER SERVER opt_native OPTIONS (ADD terminal_policy 'observe_only')")
    check(pg('SELECT id FROM opt_events WHERE id=3 ORDER BY id LIMIT 1')=='3','observe-only still confirms early limit')
    # max_block_size is a read hint: granules8192 produce16batches for both
    # requests1024/4096. Use granules512 to exercise an actual handoff reduction.
    ch('CREATE TABLE opt_fdw.large (id Int64,v String,extra String) ENGINE=MergeTree ORDER BY id SETTINGS index_granularity=512,index_granularity_bytes=0')
    ch("INSERT INTO opt_fdw.large SELECT toInt64(number),concat('event ',toString(number)),repeat('x',32) FROM numbers(131072)")
    pg("CREATE FOREIGN TABLE opt_large(id bigint,v text,extra text) SERVER opt_native OPTIONS (table 'opt_fdw.large')")
    large_query='SELECT id,v FROM opt_large ORDER BY id LIMIT 32'
    check(pg(large_query)=='\n'.join(f'{i}|event {i}' for i in range(32)),'131072-row fixture returns the exact first32 values')
    large_plan=scan(large_query,True)
    check(large_plan['FDW remote rows']==32 and large_plan['FDW metrics complete'] is True,'large ordered page transfers32 rows and completes proof')
    check(pg('SELECT count(*) FROM opt_large')=='131072','dummy projection preserves full count without aggregate pushdown')
    plans.append(large_plan)
    batch_query="SELECT count(*) FROM opt_large WHERE wc_match(v,'event *','raw')"
    default_batches=scan(batch_query,True)
    check(default_batches.get('FDW row timing enabled') is True and default_batches['FDW row conversion']>0,'EXPLAIN timing enables FDW row conversion clocks')
    untimed=scan(batch_query,True,timing=False)
    check(untimed.get('FDW row timing enabled') is False and untimed['FDW row conversion']==0,'TIMING OFF disables per-row clocks explicitly')
    check(untimed['FDW remote rows']==131072 and untimed.get('Rows Removed by Filter',0)==0 and untimed['FDW metrics complete'] is True,'TIMING OFF retains every local recheck and terminal proof')
    pg("ALTER FOREIGN TABLE opt_large OPTIONS (ADD settings 'max_block_size=4096')")
    check(pg(batch_query)=='131072','larger bounded batches preserve every matching row')
    larger_batches=scan(batch_query,True)
    check(larger_batches['FDW remote rows']==131072 and larger_batches.get('Rows Removed by Filter',0)==0,'batch tuning retains exact local rechecks')
    check(larger_batches['FDW batches']<default_batches['FDW batches'],'larger server blocks reduce batch handoffs')
    check(larger_batches['FDW metrics complete'] is True,'larger batches retain terminal confirmation')
    pg("ALTER FOREIGN TABLE opt_large OPTIONS (SET settings 'max_block_size=0')")
    check('invalid FDW setting value' in pg(batch_query,failure=True),'zero block size is rejected before dispatch')
    pg("ALTER FOREIGN TABLE opt_large OPTIONS (DROP settings)")
    plans.extend([default_batches,larger_batches])
    # The automatic policy executes only confirmed, bounded EXPLAIN probes.
    for granule in [512,8192]:
        ch(f"CREATE TABLE opt_fdw.auto{granule}(id Int64,v String,INDEX ng v TYPE text(tokenizer='ngrams')) ENGINE=MergeTree ORDER BY id SETTINGS index_granularity={granule}")
        ch(f"INSERT INTO opt_fdw.auto{granule} SELECT toInt64(number),if(number%4096=0,'abd','ordinary string') FROM numbers(65536)")
        pg(f"CREATE FOREIGN TABLE opt_auto{granule}(id bigint,v text) SERVER opt_native OPTIONS (table 'opt_fdw.auto{granule}',candidate_columns 'v',candidate_prewhere 'true',candidate_mode 'auto')")
        q=f"SELECT id FROM opt_auto{granule} WHERE wc_regex(v,'a[bc]d',false,'raw') ORDER BY id"
        check(pg(q)=='\n'.join(str(i) for i in range(0,65536,4096)),'automatic candidate plan keeps exact IDs')
        # Reuse one PostgreSQL backend so the identity-bound plan cache is exercised.
        raw=pg(f'EXPLAIN(ANALYZE,FORMAT JSON) {q}; EXPLAIN(ANALYZE,FORMAT JSON) {q}')
        decoder=json.JSONDecoder();a,end=decoder.raw_decode(raw);b,_=decoder.raw_decode(raw[end:].lstrip())
        def foreign_plan(p):
            if p['Node Type']=='Foreign Scan':return p
            for child in p.get('Plans',[]):
                found=foreign_plan(child)
                if found:return found
        first=foreign_plan(a[0]['Plan']);second=foreign_plan(b[0]['Plan'])
        check(first['FDW automatic candidate selected']==(granule==512),'auto chooses512, rejects8192')
        check(first['FDW plan probes']==2 and first['FDW plan cache hits']==0,'cold decision accounts both plan probes')
        check(second['FDW plan probes']==0 and second['FDW plan cache hits']==1,'warm decision reuses preference without repeated probes')
        check(first['FDW plan preparation']>0,'preparation cost is visible')
        check(first.get('Rows Removed by Filter',0)==0,'automatic plan has zero local removals')
        check(first['FDW HTTP body wait']>=0 and first['FDW Arrow decode']>=0,'transport and decoding clocks are exposed separately')
        check(first['FDW HTTP body bytes']>0,'HTTP entity bytes are counted')
        plans.extend([first,second])
    pg("ALTER SERVER opt_native OPTIONS (SET url 'http://127.0.0.1:1')")
    check('ClickHouse candidate SQL' in scan("SELECT id FROM opt_auto512 WHERE wc_regex(v,'a[bc]d',false,'raw')"),'auto EXPLAIN performs no network')
    check(pg("SELECT id FROM opt_auto512 WHERE wc_regex(v,'a[bc]d',false,'raw') LIMIT 0")=='','auto LIMIT0 performs no probes')
    pg(f"ALTER SERVER opt_native OPTIONS (SET url 'http://{address}:8123')")
    ch("CREATE TABLE opt_fdw.auto_memory(id Int64,v String) ENGINE=Memory")
    ch("INSERT INTO opt_fdw.auto_memory VALUES (1,'abd'),(2,'normal')")
    pg("CREATE FOREIGN TABLE opt_auto_memory(id bigint,v text) SERVER opt_native OPTIONS (table 'opt_fdw.auto_memory',candidate_columns 'v',candidate_mode 'auto')")
    q="SELECT id FROM opt_auto_memory WHERE wc_regex(v,'a[bc]d',false,'raw')"
    check(pg(q)=='1','unknown-reader automatic fallback remains exact')
    p=scan(q,True)
    check(not p['FDW automatic candidate selected'] and p['FDW plan probes']==1,'unknown baseline skips candidate probe')
    if os.environ.get('PG_OPT_IN_BAND')=='1':
        pg("ALTER SERVER opt_native OPTIONS (SET terminal_policy 'in_band')")
        q='SELECT id,v FROM opt_large ORDER BY id LIMIT 32'
        p=scan(q,True)
        check(p['FDW remote rows']==32 and p['FDW data control requests']==0,'in-band page completes without query-log confirmation')
        check(pg('SELECT id FROM opt_large WHERE id=-1')=='','in-band empty result confirms')
        plans.append(p)
        q="SELECT id FROM opt_auto512 WHERE wc_regex(v,'a[bc]d',false,'raw') ORDER BY id"
        p=scan(q,True)
        check(p['FDW remote rows']==16 and p['FDW automatic candidate selected'],'in-band automatic plan retains exact matches')
        check(p['FDW plan probes']==2 and p['FDW plan control requests']==0 and p['FDW data control requests']==0,'in-band confirms both probes and data without log controls')
        check(p['FDW plan framing decode']>0,'preparation includes its own framing cost')
        plans.append(p)
    else:
        pg("ALTER SERVER opt_native OPTIONS (SET terminal_policy 'in_band')")
        pg('SELECT id FROM opt_large ORDER BY id LIMIT 32',failure=True)
        check(True,'old engine refuses in-band mode without pretending success')
    output=os.environ.get('PG_OPT_RESULT')
    if output:
        from pathlib import Path
        Path(output).write_text(json.dumps({'postgres':pg('SHOW server_version'),'native':ch('SELECT version()'),'plans':plans,'complete':True},indent=2)+'\n')
finally:
    pg('DROP SERVER IF EXISTS opt_native CASCADE')
    pg('DROP TABLE IF EXISTS opt_local')
    pg('DROP TABLE IF EXISTS opt_null_local')
    pg('DROP SCHEMA IF EXISTS opt_custom CASCADE')
    pg('DROP SCHEMA IF EXISTS opt_collation CASCADE')
    pg('DROP SCHEMA IF EXISTS opt_security CASCADE')
    pg("DO $$ BEGIN IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname='opt_visible') THEN REVOKE ALL ON SCHEMA public FROM opt_visible; END IF; END $$")
    pg('DROP ROLE IF EXISTS opt_visible')
    ch('DROP DATABASE IF EXISTS opt_fdw')
