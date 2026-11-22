#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Generated sweep/MV benchmark. Native HTTP measures engine SQL, NOT antalya-rs.

Use --state PRIVATE.json for an owned native engine with real system.query_log;
otherwise use chDB for semantic CI smoke tests. Read generator JSON on stdin.
Each invocation owns a fresh database; never print credentials or server URL.
"""
import argparse, collections, hashlib, json, os, platform, statistics, sys, time, uuid
from pathlib import Path
import urllib.error, urllib.parse, urllib.request

SETTINGS = dict(max_threads=4, max_block_size=8192, max_insert_block_size=8192,
                min_insert_block_size_rows=8192, min_insert_block_size_bytes=0,
                max_memory_usage=4000000000, max_ast_elements=50000, max_expanded_ast_elements=500000,
                max_query_size=262144, max_result_rows=100000, max_result_bytes=67108864,
                result_overflow_mode='throw', use_query_cache=0,
                use_query_condition_cache=0, short_circuit_function_evaluation='enable')

def emit(**record):
    print(json.dumps(record, sort_keys=True), flush=True)

def machine_settings():
    cpuinfo=Path('/proc/cpuinfo')
    model=next((line.split(':',1)[1].strip() for line in cpuinfo.read_text().splitlines() if line.startswith('model name')),None) if cpuinfo.exists() else None
    meminfo=Path('/proc/meminfo')
    memory=next((int(line.split()[1])*1024 for line in meminfo.read_text().splitlines() if line.startswith('MemTotal:')),None) if meminfo.exists() else None
    return dict(platform=platform.platform(),cpu_count=os.cpu_count(),cpu_model=model,host_memory_bytes=memory,python=platform.python_version())

def redact(text, state):
    for key in ['token','url']:
        value=(state or {}).get(key)
        if value:
            text=text.replace(value,'[redacted]')
    return text

def literal(text):
    return "'" + text.replace('\\', '\\\\').replace("'", "\\'") + "'"

class Engine:
    def __init__(self, state=None, timeout_seconds=300):
        if not 1 <= timeout_seconds <= 300: raise ValueError("invalid native timeout")
        self.timeout_seconds = timeout_seconds
        self.database = 'wc_benchmark_' + uuid.uuid4().hex
        self.prefix = self.database + '_'
        self.state = json.loads(Path(state).read_text()) if state else None
        emit(kind='owned_database',database=self.database,query_prefix=self.prefix)
        self.settings = SETTINGS.copy()
        if self.state:
            self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        else:
            from chdb import session
            self.session = session.Session()
        self.query('CREATE DATABASE ' + self.database, database='default')
        if not self.state:
            self.query('USE ' + self.database)
            self.query('SET ' + ','.join(f'{k}={literal(v) if isinstance(v,str) else v}' for k,v in self.settings.items()))
    def query(self, sql, query_id=None, database=None, logging=True):
        if not self.state:
            result = str(self.session.query(sql, 'JSONCompact'))
            return json.loads(result)['data'] if result.strip() else []
        params = dict(self.settings, database=database or self.database, default_format='JSONCompact',
                      query_id=query_id or self.prefix + uuid.uuid4().hex,
                      log_queries=int(logging), max_execution_time=self.timeout_seconds)
        request = urllib.request.Request(self.state['url'] + '/?' + urllib.parse.urlencode(params),
                  data=sql.encode(), headers={'Authorization':'Bearer '+self.state['token']})
        try:
            with self.opener.open(request, timeout=self.timeout_seconds+30) as response:
                result = response.read(67108865)
                if len(result)>67108864:
                    raise RuntimeError('benchmark response exceeds 64 MiB safety cap')
        except urllib.error.HTTPError as error:
            raise RuntimeError(redact(error.read(65536).decode(errors='replace'),self.state)[-3000:]) from None
        if not result.strip():return []
        try:
            return json.loads(result)['data']
        except ValueError:
            # Streaming HTTP may send its status before a later engine error.
            detail=redact(result.decode(errors='replace'),self.state)[-3000:]
            raise RuntimeError('invalid/unfinished engine JSON response: '+detail) from None
    def profile(self, query_id):
        if not self.state:return None
        sql = "SELECT query_duration_ms,read_rows,read_bytes,result_rows,result_bytes,memory_usage,ProfileEvents,Settings FROM system.query_log WHERE type='QueryFinish' AND is_initial_query=1 AND query_id=" + literal(query_id) + ' ORDER BY event_time_microseconds DESC LIMIT 1'
        # Complete HTTP output can precede publication of QueryFinish. A flush
        # cannot expose a record that has not been queued yet. Retry control
        # reads outside the measured clock; absent terminal evidence still fails.
        for attempt in range(8):
            if attempt:time.sleep(.1)
            self.query('SYSTEM FLUSH LOGS', logging=False)
            rows = self.query(sql, logging=False)
            if rows:break
        if not rows:raise AssertionError('No QueryFinish for measured query '+query_id)
        return dict(zip(['query_duration_ms','read_rows','read_bytes','result_rows','result_bytes','memory_usage','profile_events','settings'],rows[0]))
    def close(self):
        # Cancel only this invocation's queries, including an insert timed out by the client.
        if self.state:
            self.query("KILL QUERY WHERE startsWith(query_id,"+literal(self.prefix)+") AND query_id != currentQueryID() SYNC",logging=False)
        self.query('DROP DATABASE IF EXISTS ' + self.database, database='default')

def setup(engine, spec, row_count):
    engine.query('CREATE TABLE bench (id UInt64,v String) ENGINE=MergeTree ORDER BY id')
    engine.query(f"INSERT INTO bench SELECT number,if(number%100=0,concat('https://evil',leftPad(toString(intDiv(number,100)%{spec['indicators']}),4,'0'),'.example/path'),concat('https://benign.example/path/',toString(number))) FROM numbers({row_count})")
    return (row_count + 99)//100

def inserts(engine, statements):
    prefix = statements[0].split(' VALUES ',1)[0]+' VALUES '
    for start in range(0,len(statements),128):
        engine.query(prefix+', '.join(s.split(' VALUES ',1)[1] for s in statements[start:start+128]))

def candidate_counts(engine, spec, row_count):
    # Constant scalar predicates avoid constructing row x indicator x needle arrays.
    for strategy, selected in spec['needles'].items():
        grouped = collections.Counter(tuple(needles) for needles in selected)
        expressions = []
        for needles, multiplicity in grouped.items():
            predicate = ' AND '.join('position(v,'+literal(n)+')>0' for n in needles) or '1'
            expressions.append(f'sum(toUInt64({predicate}))*{multiplicity}')
        eligible = 0
        for start in range(0,len(expressions),64):
            eligible += sum(int(x) for x in engine.query('SELECT '+','.join(expressions[start:start+64])+' FROM bench')[0])
        pairs = row_count*spec['indicators']
        emit(kind='candidate_pairs',strategy=strategy,eligible_pairs=eligible,total_pairs=pairs,eligible_fraction=eligible/pairs,
             meaning='logical precheck passes; not an instrumented count of regex invocations')

def all_indicator_canary(engine, spec, strategy):
    # The 1% timed corpus only hits its first 100 indicators. Exercise every
    # batch with one positive per indicator as an additional untimed check.
    engine.query('TRUNCATE TABLE bench')
    engine.query(f"INSERT INTO bench SELECT number,concat('https://evil',leftPad(toString(number),4,'0'),'.example/path') FROM numbers({spec['indicators']})")
    actual=[]
    for sql in spec['queries'][strategy]:
        actual.extend((int(r[0]),int(r[1]),str(r[2])) for r in engine.query(sql))
    if sorted(actual)!=[(i,i,'raw') for i in range(spec['indicators'])]:
        raise AssertionError('all-indicator canary differs')
    emit(kind='canary',strategy=strategy,indicators=spec['indicators'],hits=len(actual),timed=False)

def run(args, spec):
    engine = Engine(args.state)
    try:
        row_count = args.rows or spec['rows']
        setup_start=time.perf_counter()
        expected = setup(engine,spec,row_count)
        setup_seconds=time.perf_counter()-setup_start
        storage=engine.query("SELECT sum(rows),sum(data_compressed_bytes),sum(data_uncompressed_bytes),sum(bytes_on_disk) FROM system.parts WHERE active AND database="+literal(engine.database)+" AND table='bench'")[0]
        diagnostic=spec.get('sparse_diagnostics') if args.strategy=='sparse_candidates' else (spec.get('diagnostics') if args.strategy=='constant_candidates' else None)
        if args.mode=='sweeps' and diagnostic is None:
            diagnostic={'strategy':args.strategy,'source_scan_count':len(spec['queries'][args.strategy])}
        emit(kind='metadata',route='native_engine_http' if args.state else 'embedded_chdb',engine=engine.query('SELECT version()')[0][0],
             image=engine.state.get('image') if engine.state else None,rows=row_count,indicators=spec['indicators'],expected_hits=expected,
             machine=machine_settings(),engine_resources={key:engine.state[key] for key in ['cpus','memory_bytes']} if engine.state else None,settings=SETTINGS,
             source_load_seconds=setup_seconds,source_storage=dict(zip(['rows','compressed_bytes','uncompressed_bytes','bytes_on_disk'],map(int,storage))),
             generator_sha256=hashlib.sha256(json.dumps(spec,sort_keys=True).encode()).hexdigest(),frequency_statistics_supplied=spec.get('frequencies_supplied',False),
             cache='query/query-condition caches disabled; OS/filesystem caches uncontrolled',timing='client wall includes parsing/execution/response; engine clocks overlap',diagnostics=diagnostic)
        if args.mode == 'candidates':
            candidate_counts(engine,spec,row_count);return
        if args.mode == 'ingest':
            engine.query(spec['watchlist']['ddl'])
            inserts(engine,spec['watchlist']['inserts'].get(args.strategy,spec['watchlist']['inserts']['ranked_needles']))
            engine.query('CREATE TABLE ingest_src(id UInt64,v String) ENGINE=MergeTree ORDER BY id')
            engine.query('CREATE TABLE ingest_hits(id UInt64,indicator_id Int64,view String) ENGINE=MergeTree ORDER BY id')
            engine.query(spec['watchlist']['mvs'][args.strategy])
        samples=[]
        for repeat in range(args.repeats):
            if args.mode=='ingest':
                engine.query('TRUNCATE TABLE ingest_src');engine.query('TRUNCATE TABLE ingest_hits')
            queries=['INSERT INTO ingest_src SELECT id,v FROM bench'] if args.mode=='ingest' else spec['queries'][args.strategy]
            query_ids=[];profiles=[];matched=[];hits=0
            emit(kind='started',mode=args.mode,strategy=args.strategy,repeat=repeat,query_count=len(queries))
            started=time.perf_counter()
            for sql in queries:
                query_id=engine.prefix+uuid.uuid4().hex;query_ids.append(query_id)
                # Each timed event is logged before dispatch for recoverable timeout evidence.
                emit(kind='query_started',query_id=query_id,sql_bytes=len(sql.encode()))
                result=engine.query(sql,query_id=query_id)
                if args.mode!='ingest':matched.extend((int(r[0]),int(r[1]),str(r[2])) for r in result)
            elapsed=time.perf_counter()-started
            if args.mode=='ingest':matched=[(int(r[0]),int(r[1]),str(r[2])) for r in engine.query('SELECT id,indicator_id,view FROM ingest_hits')]
            hits=len(matched)
            expected_pairs=[(n,(n//100)%spec['indicators'],'raw') for n in range(0,row_count,100)]
            if sorted(matched)!=expected_pairs:raise AssertionError('exact row/indicator/view pairs differ')
            pair_hash=hashlib.sha256(''.join(f'{row}\t{indicator}\t{view}\n' for row,indicator,view in sorted(matched)).encode()).hexdigest()
            if hits!=expected:raise AssertionError((args.strategy,hits,expected))
            # Untimed oracle additionally checks each indicator count, not only total.
            if args.mode=='ingest':
                actual={int(k):int(v) for k,v in engine.query('SELECT indicator_id,count() FROM ingest_hits GROUP BY indicator_id')}
                oracle=collections.Counter((n//100)%spec['indicators'] for n in range(0,row_count,100))
                if actual!=dict(oracle):raise AssertionError('per-indicator mismatch')
            if args.state:
                profiles=[engine.profile(q) for q in query_ids]
            read_rows=sum(int(p['read_rows']) for p in profiles) if profiles else None
            if args.mode=='sweeps' and args.strategy in ['constant_candidates','sparse_candidates'] and spec['indicators']==1000:
                if len(queries)>2:raise AssertionError('benchmark exceeds two source queries')
                if read_rows is not None and read_rows>2*row_count:raise AssertionError('query_log exceeds two source scans')
            samples.append(elapsed)
            emit(kind='sample',mode=args.mode,strategy=args.strategy,repeat=repeat,wall_seconds=elapsed,rows_per_second=row_count/elapsed,
                 hits=hits,pair_sha256=pair_hash,query_ids=query_ids,query_count=len(queries),read_rows=read_rows,profiles=profiles)
            if elapsed>=60:break
        emit(kind='summary',mode=args.mode,strategy=args.strategy,samples=len(samples),median_seconds=statistics.median(samples),
             median_rows_per_second=row_count/statistics.median(samples))
        if args.all_indicator_canary and args.mode=='sweeps':
            all_indicator_canary(engine,spec,args.strategy)
    finally:
        original_error=sys.exc_info()[0]
        try:
            engine.close()
        except Exception as cleanup_error:
            emit(kind='cleanup_failure',error=redact(str(cleanup_error),engine.state))
            if original_error is None:
                raise

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--all-indicator-canary',action='store_true')
    p.add_argument('--state');p.add_argument('--mode',choices=['sweeps','ingest','candidates'],default='sweeps')
    p.add_argument('--strategy',default='constant_candidates');p.add_argument('--repeats',type=int,default=3)
    p.add_argument('--rows',type=int);args=p.parse_args()
    try:run(args,json.load(sys.stdin))
    except Exception as error:
        state=json.loads(Path(args.state).read_text()) if args.state and Path(args.state).exists() else None
        emit(kind='failure',mode=args.mode,strategy=args.strategy,error=redact(str(error),state))
        raise SystemExit(1) from None
