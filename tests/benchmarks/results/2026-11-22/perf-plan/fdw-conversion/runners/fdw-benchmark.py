"""Private Task4 runner. All timings sequential; controlled worker pools/barriers."""
import argparse,concurrent.futures,hashlib,json,os,statistics,subprocess,sys,threading,time,uuid,urllib.request,urllib.parse
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from compare_routes import sweeps,foreign_nodes
from route_workers import PgWorker,DirectWorker
from comparison import overlap_peak
from fdw_profile import pg_literal
from fdw_values import verify_copy
from process_sampler import SamplerProcess
ROOT=Path.cwd()
p=argparse.ArgumentParser();p.add_argument('--out',type=Path,required=True);p.add_argument('--label',required=True)
p.add_argument('--workloads',default='narrow,smalltext,wide1k,broad_recheck');p.add_argument('--concurrency',type=int,default=1)
p.add_argument('--policy',default='flush_first',choices=['flush_first','observe_first','observe_only','in_band'])
p.add_argument('--batch-bytes',type=int,default=16777216);p.add_argument('--repeats',type=int,default=5)
p.add_argument('--routes',default='direct,pg_backend,pg_copy');a=p.parse_args()
if a.concurrency not in (1,8,32) or not 1<=a.repeats<=10 or a.batch_bytes not in (1048576,4194304,16777216):raise ValueError('bounds')
workloads=a.workloads.split(',');routes=a.routes.split(',')
if set(workloads)-{'narrow','smalltext','wide1k','broad_recheck'} or set(routes)-{'direct','pg_backend','pg_copy'}:raise ValueError('selection')
if a.out.exists():raise ValueError('fresh output required')
a.out.mkdir(parents=True);state=json.load(open('.local/perf/antalya.json'))
engine=None;admin=None;pg=[];direct=[];schema='wc_fdw_'+uuid.uuid4().hex;records=[]
marker=Path('.local/perf/execution/timing-active')
if marker.exists():raise RuntimeError('another timing window active')
marker.write_text('fdw '+a.label+'\n')
def emit(r):
 with (a.out/'raw.jsonl').open('a') as f:f.write(json.dumps(r)+'\n')
 print(json.dumps({k:v for k,v in r.items() if k not in ('results','samples','profiles')}),flush=True)
def request_direct(w,sql,workload,rows,verify):
 start=time.monotonic();w.process.stdin.write((json.dumps(dict(sql=sql,workload=workload,rows=rows,verify=verify))+'\n').encode());w.process.stdin.flush();r=w.read(75)
 if 'error' in r:raise RuntimeError(r['error'])
 r['worker_roundtrip_ms']=(time.monotonic()-start)*1000;return r
try:
 engine=sweeps.Engine('.local/perf/antalya.json',timeout_seconds=60)
 engine.query('CREATE TABLE events(id Int64,url String,extra String) ENGINE=MergeTree ORDER BY id')
 engine.query("INSERT INTO events SELECT number,if(number%10000=0,'evil.test/powershell','benign.test/normal'),repeat('x',16) FROM numbers(1000000)")
 engine.query('CREATE TABLE wide_events AS events')
 engine.query("INSERT INTO wide_events SELECT number,'benign.test/normal',repeat('x',1024) FROM numbers(32768)")
 admin=PgWorker('wildcard-review-pg18',ROOT,a.out/'admin.private.log')
 admin.execute(f'CREATE SCHEMA {schema}; CREATE SERVER {schema} FOREIGN DATA WRAPPER wildcard_fdw OPTIONS(url {pg_literal(state["url"])},database {pg_literal(engine.database)},timeout_ms \'65000\',settings \'max_threads=2,max_block_size=1024,max_execution_time=60\',terminal_policy {pg_literal(a.policy)},batch_bytes {pg_literal(str(a.batch_bytes))}); CREATE USER MAPPING FOR CURRENT_USER SERVER {schema} OPTIONS(bearer_token {pg_literal(state["token"])});')
 for table in ('events','wide_events'):admin.execute(f'CREATE FOREIGN TABLE {schema}.{table}(id bigint,url text,extra text) SERVER {schema} OPTIONS(table {pg_literal(engine.database+"."+table)});')
 pg=[PgWorker('wildcard-review-pg18',ROOT,a.out/f'pg-{i}.private.log') for i in range(a.concurrency)]
 if 'direct' in routes:
  direct=[DirectWorker(ROOT/'.local/review-pg/target/release/examples/fdw_warm',state,a.out/f'direct-{i}.private.log',extra_args=[a.policy,str(a.batch_bytes)]) for i in range(a.concurrency)]
 settings=dict(label=a.label,concurrency=a.concurrency,policy=a.policy,batch_bytes=a.batch_bytes,repeats=a.repeats,workloads=workloads,routes=routes,engine=engine.query('SELECT version()')[0][0],machine=sweeps.machine_settings(),host_load=list(os.getloadavg()),pg_pids=[w.pid for w in pg],direct_setup=[dict(process_ms=w.setup_ms,**w.ready) for w in direct],pg_setup=[w.setup_ms for w in pg],timer='warm connections; request through fully written TCP COPY file or all Arrow batches; backend executor TIMING OFF separate; value checks outside timed samples',memory='sampled backend RSS/HWM/threads separate from max individual BudgetedBatch; no assertion RSS <= batch budget')
 settings['direct_worker_sha256']=hashlib.sha256((ROOT/'.local/review-pg/target/release/examples/fdw_warm').read_bytes()).hexdigest() if direct else None
 settings['runner_sha256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
 (a.out/'settings.json').write_text(json.dumps(settings,indent=2)+'\n')
 for workload in workloads:
  table='wide_events' if workload=='wide1k' else 'events';rows=32768 if workload=='wide1k' else 1000000
  projection='id' if workload=='narrow' else 'id,url,extra'
  sql=f'SELECT {projection} FROM {schema}.{table}'
  native=f'SELECT {projection} FROM {engine.database}.{table}'
  if workload=='broad_recheck':
   sql+=" WHERE wc_match(url,'*normal*','raw') OR wc_match(url,'*powershell*','raw')"
   native+=" WHERE url LIKE '%normal%' OR url LIKE '%powershell%'"
  emit(dict(kind='query',workload=workload,pg_sql=sql,direct_sql=native,rows=rows))
  # Full value controls on every connection; timing path only consumes all batches.
  for w in direct:request_direct(w,native,workload,rows,True)
  pg[0].execute('COPY ('+sql+') TO STDOUT (FORMAT binary);');verify_copy(pg[0].file.read_bytes(),workload,rows)
  def pg_finishes():
   engine.query('SYSTEM FLUSH LOGS',logging=False)
   return {row[0] for row in engine.query("SELECT query_id FROM system.query_log WHERE type='QueryFinish' AND startsWith(query_id,'wildcard-') AND position(query,"+sweeps.literal(engine.database+'.')+")>0",logging=False)}
  pg_seen=pg_finishes()
  for warmup,repeat,route in [(True,-1,r) for r in routes]+[(False,n,routes[(i+n)%len(routes)]) for n in range(a.repeats) for i in range(len(routes))]:
   barrier=threading.Barrier(a.concurrency+1,timeout=20)
   def run(i):
    barrier.wait();start=time.monotonic()
    if route=='direct':r=request_direct(direct[i],native,workload,rows,False)
    elif route=='pg_backend':r=pg[i].request(sql,True)
    else:r=dict(request_ms=pg[i].execute('COPY ('+sql+') TO STDOUT (FORMAT binary);'),backend_pid=pg[i].pid)
    return dict(worker=i,start=start,end=time.monotonic(),**r)
   sample_marker=ROOT/'.local/perf/execution'/('sampler-'+uuid.uuid4().hex);remote='/workspace/'+str(sample_marker.relative_to(ROOT))
   samples=[];active_samples=[];monitor_stop=threading.Event();monitor_errors=[]
   def monitor():
    while not monitor_stop.is_set():
     try:
      params={'default_format':'JSONCompact','log_queries':0,'max_execution_time':1}
      query="SELECT count(),sum(memory_usage) FROM system.processes WHERE query_id!=currentQueryID() AND position(query,"+sweeps.literal(engine.database+'.')+")>0"
      request=urllib.request.Request(state['url']+'/?'+urllib.parse.urlencode(params),data=query.encode(),headers={'Authorization':'Bearer '+state['token']})
      with engine.opener.open(request,timeout=2) as response:row=json.loads(response.read(65536))['data'][0]
      active_samples.append(dict(time=time.monotonic(),queries=int(row[0]),memory_bytes=int(row[1])))
     except Exception as error:monitor_errors.append(type(error).__name__)
     monitor_stop.wait(.1)
   monitor_thread=threading.Thread(target=monitor,daemon=True);monitor_thread.start();reader=None
   command=['docker','exec','wildcard-review-pg18','python3','/workspace/tests/benchmarks/process_sampler.py',remote]
   try:
    with SamplerProcess(command,sample_marker) as sampler:
     def collect():
      for line in sampler.stdout:
       samples.append(dict(time=time.monotonic(),processes=[x for x in json.loads(line) if x['pid'] in {w.pid for w in pg}]))
     reader=threading.Thread(target=collect,daemon=True);reader.start()
     with concurrent.futures.ThreadPoolExecutor(max_workers=a.concurrency) as pool:
      fs=[pool.submit(run,i) for i in range(a.concurrency)];barrier.wait();result=[f.result() for f in fs]
   finally:
    monitor_stop.set();monitor_thread.join(timeout=3)
    if reader is not None:reader.join(timeout=3)
   if monitor_thread.is_alive():raise RuntimeError('engine monitor did not stop')
   if reader is not None and reader.is_alive():raise RuntimeError('sampler did not stop')
   # Validate values after all client clocks stop, without retaining N payloads.
   for i,r in enumerate(result):
    if route=='pg_copy':r['value_control']=verify_copy(pg[i].file.read_bytes(),workload,rows)
    if route=='pg_backend':
     if r['plan']['Plan']['Actual Rows']!=rows:raise ValueError('executor row mismatch')
     r['query_ids']=[n['FDW last query ID'] for n in foreign_nodes(r['plan']['Plan'])]
   record=dict(kind='cohort',label=a.label,route=route,workload=workload,repeat=repeat,warmup=warmup,requested_concurrency=a.concurrency,observed_client_peak=overlap_peak([(r['start'],r['end']) for r in result]),cohort_ms=1000*(max(r['end'] for r in result)-min(r['start'] for r in result)),results=result,samples=samples,active_samples=active_samples,monitor_errors=monitor_errors,observed_engine_peak=max((x["queries"] for x in active_samples),default=None),host_load=list(os.getloadavg()))
   (a.out/'pending.json').write_text(json.dumps(record)+'\n')
   for r in result:
    ids=r.get('query_ids',[r['query_id']] if 'query_id' in r else [])
    r['profiles']=[engine.profile(q) for q in ids]
   if route.startswith('pg_'):
    finishes=pg_finishes();fresh=sorted(finishes-pg_seen);pg_seen=finishes
    if len(fresh)!=a.concurrency:raise ValueError('native PG correlation differs')
    if route=='pg_copy':record['profiles']=[engine.profile(q) for q in fresh];record['query_ids']=fresh
   emit(record);records.append(record);(a.out/'pending.json').unlink()
 summary=[]
 for w in workloads:
  for route in routes:
   rs=[r for c in records if c['workload']==w and c['route']==route and not c['warmup'] for r in c['results']];v=[r['request_ms'] for r in rs]
   summary.append(dict(workload=w,route=route,samples=len(v),median_ms=statistics.median(v),min_ms=min(v),max_ms=max(v),executor_median_ms=statistics.median([r['executor_ms'] for r in rs]) if route=='pg_backend' else None))
 (a.out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
finally:
 failures=[]
 for w in direct+pg:
  try:w.close()
  except Exception as error:failures.append(type(error).__name__+': worker cleanup')
 if admin:
  try:admin.execute(f'DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SERVER IF EXISTS {schema} CASCADE;')
  except Exception as error:failures.append(type(error).__name__+': schema cleanup')
  finally:
   try:admin.close()
   except Exception as error:failures.append(type(error).__name__+': admin cleanup')
 if engine:
  try:engine.close()
  except Exception as error:failures.append(type(error).__name__+': engine cleanup')
 marker.unlink(missing_ok=True)
 (a.out/'cleanup.json').write_text(json.dumps({'failures':failures})+'\n')
 if failures:raise RuntimeError('cleanup incomplete; inspect cleanup.json')
