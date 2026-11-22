#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Comparable bounded pair sweeps. Experiments/results default to private paths.

Run fixture setup and timed cohorts sequentially. Every warmup/sample is checked
against the Rust oracle after the request clock stops. No speed claims from CI.
"""
import argparse
import concurrent.futures
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import statistics
import signal
import sys
import threading
import time
import uuid
import urllib.parse
import urllib.request
from process_sampler import status as process_status
from comparison import (MAX_BYTES, MAX_PAIRS, OracleMismatch, corpus_digest, corpus_value,
                        elastic_pairs, expected_pairs, overlap_peak, require_wildcard_target,
                        schedule, verify_pairs)
from elasticsearch import Client, mapping
from fdw_profile import pg_literal
from route_workers import DirectWorker, PgWorker

ROOT=Path(__file__).resolve().parents[2]
module_spec=importlib.util.spec_from_file_location('sweeps',ROOT/'libwildcard/tests/benchmarks/sweeps.py')
sweeps=importlib.util.module_from_spec(module_spec);module_spec.loader.exec_module(sweeps)
ROUTES=('native','indexed','direct','pg_backend','pg_copy','elastic_wildcard','elastic_keyword','elastic_wildcard_4')
DEFAULT_ROUTES=tuple(route for route in ROUTES if route not in ('elastic_keyword','elastic_wildcard_4'))


def emit(out, **record):
    with (out/'raw.jsonl').open('a') as f:f.write(json.dumps(record,sort_keys=True)+'\n')
    print(json.dumps({k:v for k,v in record.items() if k not in ('profiles','results','explain','settings','spec','observations')})[:1200],flush=True)


def source_sql(spec, first, stop):
    return f"SELECT toInt64(number),if(number%{spec['stride']}=0,t.v,concat({sweeps.literal(spec['benign_prefix'])},toString(number),{sweeps.literal(spec.get('benign_suffix', ''))})) FROM numbers({first},{stop-first}) AS n INNER JOIN templates AS t ON intDiv(number,{spec['stride']})%{len(spec['templates'])}=t.id"


def storage(engine, table):
    result=engine.query("SELECT sum(rows),sum(bytes_on_disk),sum(data_compressed_bytes),sum(secondary_indices_compressed_bytes) FROM system.parts WHERE active AND database="+sweeps.literal(engine.database)+" AND table="+sweeps.literal(table))[0]
    return dict(zip(('rows','total_bytes','data_compressed_bytes','index_compressed_bytes'),map(int,result)))


def source_hash(engine, table):
    return engine.query("SELECT lower(hex(SHA256(arrayStringConcat(arrayMap(x->concat(toString(x.1),'\\t',x.2,'\\n'),arraySort(x->x.1,groupArray((id,v)))),'')))) FROM "+table)[0][0]


def native_setup(engine, spec, out, indexed):
    engine.query('CREATE TABLE templates(id UInt64,v String) ENGINE=MergeTree ORDER BY id')
    for start in range(0,len(spec['templates']),64):
        engine.query('INSERT INTO templates VALUES '+','.join(f'({i},{sweeps.literal(v)})' for i,v in enumerate(spec['templates'][start:start+64],start)))
    engine.query('CREATE TABLE bench(id Int64,v String) ENGINE=MergeTree ORDER BY id')
    split=spec['rows']*9//10
    start=time.monotonic();engine.query('INSERT INTO bench '+source_sql(spec,0,split));load=time.monotonic()-start
    index_ready=False
    if indexed:
        engine.query('CREATE TABLE bench_indexed AS bench')
        engine.query('INSERT INTO bench_indexed SELECT * FROM bench')
        start=time.monotonic()
        try:
            for ddl in spec['index_ddl']:engine.query(ddl.replace('TABLE bench ','TABLE bench_indexed '))
            # MATERIALIZE INDEX mutations must finish before settled comparisons.
            pending=engine.query("SELECT count() FROM system.mutations WHERE database="+sweeps.literal(engine.database)+" AND table='bench_indexed' AND NOT is_done")[0][0]
            deadline=time.monotonic()+60
            while int(pending):
                if time.monotonic()>deadline:raise TimeoutError('index materialization deadline')
                time.sleep(.1)
                pending=engine.query("SELECT count() FROM system.mutations WHERE database="+sweeps.literal(engine.database)+" AND table='bench_indexed' AND NOT is_done")[0][0]
            index_ready=True
            emit(out,kind='index_build',seconds=time.monotonic()-start,ddl=[d.replace('TABLE bench ','TABLE bench_indexed ') for d in spec['index_ddl']])
        except Exception as error:
            emit(out,kind='index_unavailable',error=sweeps.redact(str(error),engine.state))
    costs={}
    for table in ['bench']+(['bench_indexed'] if index_ready else []):
        start=time.monotonic();engine.query('INSERT INTO '+table+' '+source_sql(spec,split,spec['rows']));costs[table]=time.monotonic()-start
        engine.query('OPTIMIZE TABLE '+table+' FINAL')
        checksum=source_hash(engine,table)
        if checksum!=corpus_digest(spec):raise ValueError('native source corpus digest differs')
        emit(out,kind='source',table=table,initial_load_seconds=load if table=='bench' else None,incremental_insert_seconds=costs[table],storage=storage(engine,table),corpus_sha256=checksum)
    return index_ready


def elastic_setup(client, index, kind, spec, out):
    metadata={'owner':index,'corpus_sha256':corpus_digest(spec),'pattern_sha256':spec['pattern_sha256']}
    ddl=mapping(kind,metadata);client.request('PUT','/'+index,ddl)
    # IDs and raw strings are generated from the very same versioned fixture.
    start=time.monotonic();split=spec['rows']*9//10;initial=None
    for first in range(0,spec['rows'],2000):
        lines=[]
        for row in range(first,min(first+2000,spec['rows'])):
            lines.extend((json.dumps({'index':{'_id':str(row)}}),json.dumps({'id':row,'v':corpus_value(spec,row)})))
        response=client.request('POST','/'+index+'/_bulk','\n'.join(lines)+'\n',True)
        if response.get('errors') or len(response.get('items',[]))!=len(lines)//2:raise ValueError('Elastic bulk failure')
        if first+2000>=split and initial is None:initial=time.monotonic()-start
    loaded=time.monotonic()-start
    client.request('POST','/'+index+'/_refresh')
    client.request('POST','/'+index+'/_forcemerge?max_num_segments=1')
    if client.request('GET','/'+index+'/_count')['count']!=spec['rows']:raise ValueError('Elastic source count differs')
    stats=client.request('GET','/'+index+'/_stats/store,docs,segments')
    emit(out,kind='elastic_source',field_type=kind,index=index,load_seconds=loaded,initial_load_seconds=initial,incremental_insert_seconds=loaded-initial,ddl=ddl,storage=stats,corpus_sha256=metadata['corpus_sha256'])


def elastic_run(client,index,spec,search_concurrency=1):
    if search_concurrency not in (1,4):raise ValueError('invalid Elastic internal concurrency')
    pairs=[];server=[];start=time.monotonic();responses=[];retained_hits=0
    for first in range(0,len(spec['patterns']),100):
        if time.monotonic()-start>60:raise TimeoutError('Elastic sweep deadline')
        chunk=spec['patterns'][first:first+100];lines=[]
        for pattern in chunk:
            lines.extend((json.dumps({'index':index}),json.dumps({'size':10000,'track_total_hits':True,'timeout':'60s','_source':['id'], 'query':{'wildcard':{'v':{'value':pattern['elastic_wildcard'],'case_insensitive':False}}}})))
        result=client.request('POST','/_msearch?max_concurrent_searches='+str(search_concurrency),'\n'.join(lines)+'\n',True)
        batch=result.get('responses',[])
        if len(batch)!=len(chunk):raise OracleMismatch('missing Elastic response')
        if any('error' in r or r.get('timed_out',True) or r.get('_shards',{}).get('failed',1) for r in batch):raise RuntimeError('Elastic search failed or timed out')
        retained_hits+=sum(len(r['hits']['hits']) for r in batch)
        if retained_hits>MAX_PAIRS:raise ValueError('Elastic aggregate pair bound')
        responses.append((batch,chunk))
    elapsed=(time.monotonic()-start)*1000
    if elapsed>60000:raise TimeoutError('Elastic sweep completed after deadline')
    # The correctness pass and sorting/hashing are outside the common clock.
    for result,chunk in responses:
        pairs.extend(elastic_pairs(result,chunk));server.extend(r.get('took') for r in result)
        if len(pairs)>MAX_PAIRS:raise ValueError('Elastic aggregate result bound')
    return {'request_ms':elapsed,'pairs':pairs,'server_query_ms':server,'source_queries':len(spec['patterns']),'msearch_requests':len(responses),'msearch_max_concurrent_searches':search_concurrency}


def native_run(engine,queries):
    start=time.monotonic();pairs=[];ids=[]
    for query in queries:
        if time.monotonic()-start>60:raise TimeoutError('native sweep deadline')
        qid=engine.prefix+uuid.uuid4().hex;ids.append(qid)
        pairs.extend(engine.query(query,query_id=qid))
        if len(pairs)>MAX_PAIRS:raise ValueError('native aggregate pair bound')
    elapsed=(time.monotonic()-start)*1000
    if elapsed>60000:raise TimeoutError('native sweep completed after deadline')
    return {'request_ms':elapsed,'pairs':pairs,'query_ids':ids,'source_queries':len(queries)}


def foreign_nodes(plan):
    if plan.get('Node Type')=='Foreign Scan':yield plan
    for child in plan.get('Plans',[]):yield from foreign_nodes(child)


def cohorts(route, workers, action, expected, out, warmup, repeat, observe=None):
    stop=threading.Event();observations=[];observation_errors=[]
    def sample():
        while not stop.is_set():
            try: observations.append(observe())
            except Exception as error: observation_errors.append(type(error).__name__)
            stop.wait(.1)
    observer=threading.Thread(target=sample,daemon=True) if observe else None
    if observer:observer.start()
    barrier=threading.Barrier(workers+1,timeout=20)
    def run(worker):
        barrier.wait();start=time.monotonic()
        try:
            result=action(worker)
            end=time.monotonic()
            if 'pairs' in result:
                result['pair_sha256']=verify_pairs([tuple(p) for p in result.pop('pairs')],expected)
                result['pairs']=len(expected)
            return {'worker':worker,'start':start,'end':end,**result}
        except Exception as error:return {'worker':worker,'start':start,'end':time.monotonic(),'error':str(error),'correctness_failure':isinstance(error,OracleMismatch)}
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
            futures=[pool.submit(run,i) for i in range(workers)]
            barrier.wait();results=[f.result() for f in futures]
    finally:
        stop.set()
        if observer:
            observer.join(timeout=3)
            if observer.is_alive():raise RuntimeError('measurement sampler did not stop')
    record={'route':route,'warmup':warmup,'repeat':repeat,'requested_concurrency':workers,
            'observed_client_peak':overlap_peak([(r['start'],r['end']) for r in results]),
            'cohort_seconds':max(r['end'] for r in results)-min(r['start'] for r in results),'results':results,
            'observations':observations,'sampling_errors':observation_errors,'sampler_period_ms':100}
    return record


def cancel_native_queries(engine):
    engine.query("KILL QUERY WHERE query_id != currentQueryID() AND (startsWith(query_id,"+sweeps.literal(engine.prefix)+") OR (database="+sweeps.literal(engine.database)+" AND (startsWith(query_id,'comparison-direct-') OR startsWith(query_id,'wildcard-')))) SYNC",logging=False)


def pg_precontrol(workers, sql, expected, canary_sql, canary_expected, cancel):
    """A bounded PG precheck can censor PG routes, never correctness failures."""
    for index, worker in enumerate(workers):
        stage='full_copy'
        try:
            proof=worker.request(sql);verify_pairs(proof['pairs'],expected)
            stage='canary'
            proof=worker.request(canary_sql);verify_pairs(proof['pairs'],canary_expected)
        except TimeoutError as error:
            for session in workers:session.close()
            cancel()
            return {'stage':stage,'worker':index,'error':str(error),'cleanup_confirmed':True}
    return None


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('spec',type=Path);parser.add_argument('--native-state',type=Path,required=True)
    parser.add_argument('--elastic-state',type=Path);parser.add_argument('--pg-container')
    parser.add_argument('--direct-worker',type=Path);parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--routes');parser.add_argument('--repeats',type=int,default=5)
    parser.add_argument('--concurrency',type=int,default=1)
    parser.add_argument('--elastic-searches',type=int,choices=(1,4),default=1,help='Bounded searches within each msearch; separate from concurrent client sweeps')
    parser.add_argument('--layout',choices=('default','byte_dispatch'),default='default')
    parser.add_argument('--ingest-durability',choices=('defaults','fsync'),default='fsync',help='Byte layout only: fsync aligns configured acknowledgment durability; defaults records unequal engine defaults')
    args=parser.parse_args();spec=json.loads(args.spec.read_text())
    byte_layout=args.layout=='byte_dispatch'
    if byte_layout:
        import indexed_ingest
        spec=indexed_ingest.select_spec(spec)
    require_wildcard_target(spec)
    expected=expected_pairs(spec)
    defaults=tuple(r for r in DEFAULT_ROUTES if r!='indexed')+('elastic_wildcard_4',) if byte_layout else DEFAULT_ROUTES
    routes=args.routes.split(',') if args.routes else list(defaults)
    if byte_layout and ('indexed' in routes or 'elastic_keyword' in routes):
        raise ValueError('byte_dispatch layout uses one shared indexed source and the Elastic wildcard mapping')
    if set(routes)-set(ROUTES) or args.concurrency not in (1,4,8,32):raise ValueError('invalid routes/concurrency')
    schedule(routes,args.repeats)
    if args.out.exists():raise ValueError('fresh result directory required')
    args.out.mkdir(parents=True);out=args.out
    (out/'spec.json').write_text(json.dumps(spec,sort_keys=True)+'\n')
    engine=None;elastic=None;indexes={};direct=[];pg=[];admin=None;schema=None
    pg_setup=[];precontrol_failure=None
    private=json.loads(args.native_state.read_text());pg_state={'schema':None}
    try:
        engine=sweeps.Engine(args.native_state,timeout_seconds=60)
        if byte_layout:
            indexed_ingest.native_setup(engine,spec,out,emit=emit,storage=storage,source_hash=source_hash,literal=sweeps.literal,durability=args.ingest_durability)
            index_ready=False
        else:
            index_ready=native_setup(engine,spec,out,'indexed' in routes)
        if not index_ready:routes=[r for r in routes if r!='indexed']
        queries=[q.replace('FROM bench\n','FROM '+engine.database+'.bench\n') for q in spec['queries']]
        indexed=[q.replace('FROM bench\n','FROM '+engine.database+'.bench_indexed\n') for q in spec['indexed_queries']]
        engine.query('CREATE TABLE canary AS bench')
        engine.query('INSERT INTO canary SELECT toInt64(id),v FROM templates')
        canary_queries=[q.replace(engine.database+'.bench',engine.database+'.canary') for q in queries]
        canary_expected=[(row,int(hit[0]),str(hit[1])) for row,hits in enumerate(spec['template_hits']) for hit in hits]
        verify_pairs([tuple(p) for p in native_run(engine,canary_queries)['pairs']],canary_expected)
        emit(out,kind='canary',route='native',pairs=len(canary_expected),all_indicators=len({p[1] for p in canary_expected}),timed=False)
        (out/'queries.json').write_text(json.dumps({'native':queries,'indexed':indexed},indent=2)+'\n')
        for label,sqls in [('native',queries)]+([('indexed',indexed)] if index_ready else []):
            plans=[engine.query('EXPLAIN indexes=1 '+q) for q in sqls]
            (out/(label+'-explain.json')).write_text(json.dumps(plans,indent=2)+'\n')
        if any(r.startswith('elastic_') for r in routes):
            if spec['elastic_supported']:
                elastic=Client(json.loads(args.elastic_state.read_text()),70,'wc-compare-'+uuid.uuid4().hex)
                for kind in ('wildcard','keyword'):
                    if 'elastic_'+kind not in routes and not (kind=='wildcard' and 'elastic_wildcard_4' in routes):continue
                    index='wc_compare_'+uuid.uuid4().hex;indexes[kind]=index
                    if byte_layout:indexed_ingest.elastic_setup(elastic,index,kind,spec,out,emit=emit,durability=args.ingest_durability)
                    else:elastic_setup(elastic,index,kind,spec,out)
            else:
                emit(out,kind='excluded',route='elastic',reason=spec['elastic_exclusion'])
                routes=[r for r in routes if not r.startswith('elastic_')]
        if 'direct' in routes:
            if not args.direct_worker:raise ValueError('direct worker required')
            for i in range(args.concurrency):
                direct.append(DirectWorker(args.direct_worker.resolve(),private,out/f'direct-{i}.private.log'))
                verify_pairs([tuple(p) for p in direct[-1].request(engine.database,canary_queries)['pairs']],canary_expected)
            emit(out,kind='canary',route='direct',pairs=len(canary_expected),timed=False)
        pgsql=None
        if any(r.startswith('pg_') for r in routes):
            if not args.pg_container:raise ValueError('owned PG container required')
            admin=PgWorker(args.pg_container,ROOT,out/'pg-admin.private.log')
            schema='wc_compare_'+uuid.uuid4().hex;pg_state['schema']=schema
            ddl=f'CREATE SCHEMA {schema}; CREATE SERVER {schema} FOREIGN DATA WRAPPER wildcard_fdw OPTIONS(url {pg_literal(private["url"])},database {pg_literal(engine.database)},timeout_ms \'65000\',settings \'max_threads=4,max_block_size=8192,max_execution_time=60\',terminal_policy \'flush_first\'); CREATE USER MAPPING FOR CURRENT_USER SERVER {schema} OPTIONS(bearer_token {pg_literal(private["token"])});'
            admin.execute(ddl)
            def pg_tables(sqls,suffix):
                selects=[]
                for i,q in enumerate(sqls):
                    table=f'{schema}.{suffix}_{i}'
                    admin.execute(f'CREATE FOREIGN TABLE {table}(id bigint,indicator_id bigint,view text) SERVER {schema} OPTIONS(table {pg_literal("("+q+") AS sweep_pairs")});')
                    selects.append('SELECT id,indicator_id,view FROM '+table)
                return 'SELECT * FROM ('+' UNION ALL '.join(selects)+') AS all_pairs LIMIT 100001'
            pgsql=pg_tables(queries,'batch');canary_pgsql=pg_tables(canary_queries,'canary')
            for i in range(args.concurrency):pg.append(PgWorker(args.pg_container,ROOT,out/f'pg-{i}.private.log'))
            pg_setup=[{'process_ms':w.setup_ms,'backend_pid':w.pid} for w in pg]
            precontrol_failure=pg_precontrol(pg,pgsql,expected,canary_pgsql,canary_expected,lambda:cancel_native_queries(engine))
            if precontrol_failure:
                precontrol_failure['routes']=[r for r in routes if r.startswith('pg_')]
                emit(out,kind='precontrol_failure',timed=False,**precontrol_failure)
                pg=[]
            else:
                emit(out,kind='canary',route='postgres',pairs=len(canary_expected),timed=False)
        versions={'postgres':None,'elastic':None}
        if admin:
            admin.execute('SELECT version();');versions['postgres']=admin.file.read_text().strip()
        if elastic:versions['elastic']=elastic.request('GET','/')['version']['number']
        metadata={'versions':versions,'schema':'wildcard-route-measurement-v1','engine':engine.query('SELECT version()')[0][0],
                  'engine_image':private.get('image'),'engine_resources':{k:private.get(k) for k in ('cpus','memory_bytes')},'machine':sweeps.machine_settings(),'host_load':list(os.getloadavg()),'settings':engine.settings,
                  'corpus_sha256':corpus_digest(spec),'spec_sha256':hashlib.sha256(args.spec.read_bytes()).hexdigest(),
                  'runner_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'direct_worker_sha256':hashlib.sha256(args.direct_worker.read_bytes()).hexdigest() if args.direct_worker else None,'pattern_sha256':spec['pattern_sha256'],'rows':spec['rows'],'indicators':spec['indicators'],
                  'routes':routes,'repeats':args.repeats,'concurrency':args.concurrency,
                  'elastic_msearch_max_concurrent_searches':args.elastic_searches,
                  'elastic_fixture':{k:v for k,v in json.loads(args.elastic_state.read_text()).items() if k in ('image','cpus','memory_bytes','heap_bytes','version')} if elastic else None,
                  'native_max_threads_per_query':4,
                  'layout':args.layout,'ingest_durability':args.ingest_durability if byte_layout else 'legacy_defaults',
                  'elastic_route_concurrency':{r:4 if r=='elastic_wildcard_4' else args.elastic_searches for r in routes if r.startswith('elastic_')},
                  'direct_setup':[{'process_ms':w.setup_ms,**w.ready} for w in direct],
                  'pg_setup':pg_setup,
                  'precontrol_failure':precontrol_failure,
                  'timer':'request through complete consumption; exact validation/hash after timing; PG backend executor separate from EXPLAIN request; direct/PG sessions reused',
                  'cache':'one untimed warmup; native/direct query result and condition caches disabled; PG inherits the fixture cache policy (inspect native query profiles); Elastic request/query caches disabled; text-index and filesystem caches uncontrolled',
                  'failures':'60s per server query; sweeps completed after60s are rejected even on the last response. HTTP idle/client deadlines bound overdue work; cancellation is confirmed before the next variant. First failure censors remaining samples of that route.'}
        elastic_dsl=[{'id':p['id'],'view':p['view'],'query':{'wildcard':{'v':{'value':p['elastic_wildcard'],'case_insensitive':False}}}} for p in spec['patterns']] if spec['elastic_supported'] else []
        (out/'elastic-dsl.json').write_text(json.dumps(elastic_dsl,indent=2)+'\n')
        (out/'settings.json').write_text(json.dumps(metadata,indent=2)+'\n')
        # This database is unique to this run. Attribute full-COPY queries by
        # fresh terminal query IDs; never infer server work from EXPLAIN estimates.
        def pg_finishes():
            engine.query('SYSTEM FLUSH LOGS',logging=False)
            return {row[0] for row in engine.query("SELECT query_id FROM system.query_log WHERE type='QueryFinish' AND startsWith(query_id,'wildcard-') AND position(query,"+sweeps.literal(engine.database+'.bench')+")>0",logging=False)}
        pg_seen=pg_finishes() if pg else set()
        def observe(route):
            processes=[process_status(w.process.pid) for w in direct] if route=='direct' else []
            if route.startswith('elastic_'):
                monitor=Client(json.loads(args.elastic_state.read_text()),2)
                nodes=monitor.request('GET','/_nodes/stats/indices,process,jvm')['nodes'].values()
                return {'sample_time':time.monotonic(),'active_queries':sum(n['indices']['search']['query_current'] for n in nodes),
                        'heap_bytes':sum(n['jvm']['mem']['heap_used_in_bytes'] for n in nodes),
                        'cpu_ms':sum(n['process']['cpu']['total_in_millis'] for n in nodes)}
            params={'query_id':'comparison-monitor-'+uuid.uuid4().hex,'log_queries':0,'max_execution_time':1,'default_format':'JSONCompact'}
            sql="SELECT count(),sum(memory_usage) FROM system.processes WHERE query_id!=currentQueryID() AND (database="+sweeps.literal(engine.database)+" OR position(query,"+sweeps.literal(engine.database+'.bench')+")>0)"
            request=urllib.request.Request(private['url']+'/?'+urllib.parse.urlencode(params),data=sql.encode(),headers={'Authorization':'Bearer '+private['token']})
            with engine.opener.open(request,timeout=2) as response:
                raw=response.read(65537)
                if len(raw)>65536:raise ValueError('monitor response bound')
                row=json.loads(raw)['data'][0]
            return {'sample_time':time.monotonic(),'active_queries':int(row[0]),'query_memory_bytes':int(row[1]),'client_processes':processes}
        disabled=set(precontrol_failure['routes'] if precontrol_failure else []);all_records=[]
        for warmup,repeat,route in schedule(routes,args.repeats):
            if route in disabled:continue
            def action(worker):
                if route in ('native','indexed'):return native_run(engine,indexed if route=='indexed' else queries)
                if route=='direct':return direct[worker].request(engine.database,queries)
                if route.startswith('elastic_'):return elastic_run(elastic,indexes['wildcard' if route=='elastic_wildcard_4' else route[8:]],spec,4 if route=='elastic_wildcard_4' else args.elastic_searches)
                result=pg[worker].request(pgsql,backend=route=='pg_backend')
                if route=='pg_backend':
                    nodes=list(foreign_nodes(result['plan']['Plan']))
                    if sum(n['Actual Rows']*n['Actual Loops'] for n in nodes)!=len(expected):raise OracleMismatch('backend row count differs')
                    result['query_ids']=[n['FDW last query ID'] for n in nodes]
                    result['oracle']='identical SQL full COPY validated before measurements; executor consumes rows without wire delivery'
                return result
            record=cohorts(route,args.concurrency,action,expected,out,warmup,repeat,lambda:observe(route))
            # Persist measured evidence before query-log control requests.
            (out/'pending.json').write_text(json.dumps(record)+'\n')
            if any('error' in r for r in record['results']):
                # Finish cancellation before another timed variant. The database
                # and opaque ID belong only to this invocation.
                cancel_native_queries(engine)
                if route=='direct':
                    for worker in direct:worker.close()
                    direct=[]
                if route.startswith('pg_'):
                    for worker in pg:worker.close()
                    pg=[];disabled.update(('pg_backend','pg_copy'))
                if route.startswith('elastic_'):
                    tasks=elastic.request('GET','/_tasks?actions=indices:data/read/*search*&detailed=true&group_by=none').get('tasks',[])
                    for task in tasks:
                        if task.get('headers',{}).get('X-Opaque-Id')==elastic.opaque_id:
                            elastic.request('POST','/_tasks/'+task['node']+':'+str(task['id'])+'/_cancel?wait_for_completion=true')
                    remaining=elastic.request('GET','/_tasks?actions=indices:data/read/*search*&detailed=true&group_by=none').get('tasks',[])
                    if any(t.get('headers',{}).get('X-Opaque-Id')==elastic.opaque_id for t in remaining):raise RuntimeError('owned Elastic cancellation incomplete; stop timing')
                record['cleanup_confirmed']=True
            fresh_pg=[]
            if route.startswith('pg_'):
                finishes=pg_finishes();fresh_pg=sorted(finishes-pg_seen);pg_seen=finishes
            for result in record['results']:
                if 'error' in result:
                    result['error']=sweeps.redact(result['error'],private);disabled.add(route);continue
                qids=result.get('query_ids',[e['query_id'] for e in result.get('executions',[])])
                result['profiles']=[engine.profile(q) for q in qids]
            if route=='pg_copy' and not any('error' in r for r in record['results']):
                if len(fresh_pg)!=len(queries)*args.concurrency:raise ValueError('COPY native query correlation differs')
                record['query_ids']=fresh_pg;record['profiles']=[engine.profile(q) for q in fresh_pg]
            record['host_load']=list(os.getloadavg());all_records.append(record)
            emit(out,kind='cohort',**record);(out/'pending.json').unlink()
        summary=[]
        for route in routes:
            records=[r for c in all_records if c['route']==route and not c['warmup'] for r in c['results'] if 'error' not in r]
            values=[r['request_ms'] for r in records]
            summary.append({'route':route,'samples':len(values),'median_ms':statistics.median(values) if values else None,
                            'min_ms':min(values) if values else None,'max_ms':max(values) if values else None,'censored':route in disabled,
                            'observed_engine_peak':max((o['active_queries'] for c in all_records if c['route']==route for o in c['observations']),default=None),
                            'sampler_errors':sum(len(c['sampling_errors']) for c in all_records if c['route']==route)})
        (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
        if any(r.get('correctness_failure') for c in all_records for r in c['results']):
            raise OracleMismatch('a route violated the exact pair oracle; see raw records')
    finally:
        primary_error=sys.exc_info()[0];cleanup_errors=[]
        for worker in direct+pg:
            try:worker.close()
            except Exception as error:cleanup_errors.append(type(error).__name__)
        if admin:
            try:
                if admin.process.poll() is not None:
                    admin.close();admin=PgWorker(args.pg_container,ROOT,out/'pg-cleanup.private.log')
                if schema:admin.execute(f'DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SERVER IF EXISTS {schema} CASCADE;')
            except Exception as error:cleanup_errors.append(type(error).__name__)
            finally:
                try:admin.close()
                except Exception as error:cleanup_errors.append(type(error).__name__)
        if elastic:
            for index in indexes.values():
                try:elastic.request('DELETE','/'+index)
                except Exception as error:cleanup_errors.append(type(error).__name__)
        if engine:
            try:engine.close()
            except Exception as error:cleanup_errors.append(type(error).__name__)
        if cleanup_errors:
            emit(out,kind='cleanup_failure',errors=cleanup_errors)
            if primary_error is None:raise RuntimeError('cleanup failed; do not start another timed case')

if __name__=='__main__':
    def terminate(_signum,_frame):raise KeyboardInterrupt('termination requested')
    signal.signal(signal.SIGTERM,terminate)
    main()
