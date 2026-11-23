"""Owned backend CPU samples. Profiling is separate from latency measurements."""
import argparse,json,sys,time,uuid,subprocess,hashlib,signal
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from compare_routes import sweeps,foreign_nodes
from route_workers import PgWorker
from fdw_profile import pg_literal
from fdw_perf import start_owned,stop_owned
from fdw_values import verify_copy
parser=argparse.ArgumentParser();parser.add_argument('--label',choices=['baseline','optimized'],required=True);a=parser.parse_args()
root=Path.cwd();out=root/'.local/followups-20261123/flames'/a.label
if out.exists():raise RuntimeError('fresh output required')
out.mkdir(parents=True)
def terminate(_signum,_frame):raise KeyboardInterrupt('owned CPU profile termination')
signal.signal(signal.SIGTERM,terminate)
marker=root/'.local/perf/execution/timing-active'
if marker.exists():raise RuntimeError('another measurement active')
marker.write_text('fdw CPU profiles\n')
state=json.loads(Path('.local/followups-20261123/antalya.private.json').read_text());schema='wc_flame_'+uuid.uuid4().hex
engine=admin=worker=None;owned=[];failures=[]
try:
 engine=sweeps.Engine('.local/followups-20261123/antalya.private.json',timeout_seconds=60)
 binary=subprocess.check_output(['docker','exec','wildcard-review-pg18','sha256sum','/usr/lib/postgresql/18/lib/pg_wildcard.so'],text=True,timeout=15).split()[0]
 pg_version=subprocess.check_output(['docker','exec','wildcard-review-pg18','/usr/lib/postgresql/18/bin/postgres','--version'],text=True,timeout=15).strip()
 provenance=dict(label=a.label,binary_sha256=binary,postgres=pg_version,engine=engine.query('SELECT version()')[0][0],engine_settings=engine.settings,machine=sweeps.machine_settings(),runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),attribution='backend process CPU samples; not full-client latency or exclusive stage durations')
 symroot=root/'.local/followups-20261123/symbol-roots'/a.label
 binary_name='baseline.so' if a.label=='baseline' else 'candidate.so'
 mappings={'usr/lib/postgresql/18/lib/pg_wildcard.so':'/workspace/.local/followups-20261123/'+binary_name}
 for item in ('usr/lib/postgresql/18/bin/postgres','usr/lib/x86_64-linux-gnu/libc.so.6','usr/lib/x86_64-linux-gnu/libm.so.6','usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2'):
  mappings[item]='/proc/1/root/'+item
 for item,target in mappings.items():
  link=symroot/item;link.parent.mkdir(parents=True,exist_ok=True)
  if link.is_symlink():
   if str(link.readlink())!=target:raise ValueError('unexpected symbol mapping')
  elif link.exists():raise ValueError('unexpected symbol file')
  else:link.symlink_to(target)
 provenance['symbol_mappings']=mappings
 provenance['symbolization']='perf --no-inline with exact saved ELF: real DWARF-unwound frames; no synthetic inline expansion'
 provenance['perf_version']=subprocess.check_output(['docker','exec','wildcard-perf-plan-helper','perf','version'],text=True,timeout=10).strip()
 (out/'provenance.json').write_text(json.dumps(provenance,indent=2)+'\n')
 engine.query('CREATE TABLE events(id Int64,url String,extra String) ENGINE=MergeTree ORDER BY id')
 engine.query("INSERT INTO events SELECT number,if(number%10000=0,'evil.test/powershell','benign.test/normal'),repeat('x',16) FROM numbers(1000000)")
 engine.query('CREATE TABLE wide_events AS events')
 engine.query("INSERT INTO wide_events SELECT number,'benign.test/normal',repeat('x',1024) FROM numbers(32768)")
 admin=PgWorker('wildcard-review-pg18',root,out/'admin.private.log')
 admin.execute(f"CREATE SCHEMA {schema}; CREATE SERVER {schema} FOREIGN DATA WRAPPER wildcard_fdw OPTIONS(url {pg_literal(state['url'])},database {pg_literal(engine.database)},terminal_policy 'in_band',batch_bytes '16777216',settings 'max_threads=2,max_block_size=1024,max_execution_time=60'); CREATE USER MAPPING FOR CURRENT_USER SERVER {schema} OPTIONS(bearer_token {pg_literal(state['token'])});")
 for table in ('events','wide_events'):
  admin.execute(f'CREATE FOREIGN TABLE {schema}.{table}(id bigint,url text,extra text) SERVER {schema} OPTIONS(table {pg_literal(engine.database+"."+table)});')
 for workload in ['narrow','smalltext','wide1k','broad_recheck']:
  directory=out/workload;directory.mkdir();remote='/workspace/'+str(directory.relative_to(root))
  worker=PgWorker('wildcard-review-pg18',root,directory/'pg.private.log')
  table='wide_events' if workload=='wide1k' else 'events';rows=32768 if workload=='wide1k' else 1000000
  projection='id' if workload=='narrow' else 'id,url,extra'
  sql=f'SELECT {projection} FROM {schema}.{table}'
  if workload=='broad_recheck':sql+=" WHERE wc_match(url,'*normal*','raw') OR wc_match(url,'*powershell*','raw')"
  worker.execute('COPY ('+sql+') TO STDOUT (FORMAT binary);');control=verify_copy(worker.file.read_bytes(),workload,rows)
  def cpu_ticks():
   code="import json,os; s=open('/proc/%d/stat').read().rsplit(')',1)[1].split(); print(json.dumps({'user_ticks':int(s[11]),'system_ticks':int(s[12]),'hz':os.sysconf('SC_CLK_TCK')}))" % worker.pid
   return json.loads(subprocess.check_output(['docker','exec','wildcard-review-pg18','python3','-c',code],text=True,timeout=10))
  plans=[]
  with (directory/'perf-record.log').open('w') as log:
   handle=start_owned('wildcard-perf-plan-helper',['perf','record','-e','cycles:u','-F','199','--call-graph','dwarf,16384','-p',str(worker.pid),'-o',remote+'/cpu.perf.data'],stdout=log,stderr=subprocess.STDOUT)
   owned.append(handle);time.sleep(.5)
   if handle[0].poll() is not None:raise RuntimeError('perf failed to attach; inspect log')
   before_cpu=cpu_ticks()
   for _ in range(20):
    result=worker.request(sql,True)
    if result['plan']['Plan']['Actual Rows']!=rows:raise AssertionError('profile row count')
    plans.append(result)
   after_cpu=cpu_ticks()
   cpu=dict(user_seconds=(after_cpu['user_ticks']-before_cpu['user_ticks'])/before_cpu['hz'],system_seconds=(after_cpu['system_ticks']-before_cpu['system_ticks'])/before_cpu['hz'],rows=rows*20,clock_ticks_per_second=before_cpu['hz'],scope='backend process thread-group CPU during 20 profiled scans; separate from unprofiled wall-time cohorts')
   (directory/'cpu.json').write_text(json.dumps(cpu,indent=2)+'\n')
   worker.close();worker=None
   handle[0].wait(timeout=30)
   if handle[0].returncode:raise RuntimeError('perf recording failed')
  (directory/'plans.json').write_text(json.dumps(plans,indent=2)+'\n')
  for command,name in [('script','perf.script'),('report','perf-report.txt')]:
   args=['docker','exec','wildcard-perf-plan-helper','perf',command,'--no-inline','-i',remote+'/cpu.perf.data','--symfs','/workspace/'+str(symroot.relative_to(root))]
   if command=='report':args+=['--stdio','--percent-limit','1']
   with (directory/name).open('w') as output:subprocess.run(args,stdout=output,check=True,timeout=60)
  with (directory/'perf.script').open() as inp,(directory/'perf-demangled.script').open('w') as output:
   subprocess.run(['c++filt','-s','rust'],stdin=inp,stdout=output,check=True,timeout=30)
  subprocess.run(['python3','tests/benchmarks/perf_flamegraph.py',str(directory/'perf-demangled.script'),str(directory/'flamegraph.svg')],check=True,timeout=30)
  (directory/'settings.json').write_text(json.dumps(dict(workload=workload,sql=sql,value_control=control,repetitions=20,policy='in_band',batch_bytes=16777216,profile='cycles:u 199Hz DWARF16384; perf --no-inline;  sampled backend process only, inclusive attribution; clocks not comparable to unprofiled latency'),indent=2)+'\n')
  print('PROFILE COMPLETE',workload,flush=True)
finally:
 for handle in reversed(owned):failures.extend(stop_owned(handle))
 if worker:
  try:worker.close()
  except Exception:failures.append('worker cleanup')
 if admin:
  try:admin.execute(f'DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SERVER IF EXISTS {schema} CASCADE;')
  except Exception:failures.append('schema cleanup')
  finally:admin.close()
 if engine:engine.close()
 marker.unlink(missing_ok=True)
 (out/'cleanup.json').write_text(json.dumps(dict(failures=failures))+'\n')
 if failures:raise RuntimeError('profile cleanup incomplete')
