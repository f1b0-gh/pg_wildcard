"""Paired materialized-view insertion, identical patterns/values, changed needles only."""
import argparse,json,sys,time,statistics,os,uuid,signal
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from compare_routes import sweeps,native_setup
from comparison import expected_pairs,verify_pairs,schedule,require_wildcard_target
parser=argparse.ArgumentParser();parser.add_argument('--rows',type=int,choices=[10000,1000000],default=1000000);parser.add_argument('--repeats',type=int,default=5);parser.add_argument('--out',type=Path,default=Path('.local/perf/anchored-needles/runs'));parser.add_argument('--timeout-seconds',type=int,choices=[60,300],default=60);a=parser.parse_args()
if not 1<=a.repeats<=5:raise ValueError('repetition bound')
def terminate(_signum,_frame):raise KeyboardInterrupt('owned benchmark termination')
signal.signal(signal.SIGTERM,terminate)
root=Path('.local/perf/anchored-needles');a.out.mkdir(exist_ok=False)
status=[]
for family in ['domains','paths','urls','commands']:
 old=json.loads(Path(f'.local/perf/scoped-specs/{family}-100-1000000.json').read_text())
 new=json.loads((root/'specs'/f'{family}-100-1000000.json').read_text())
 for spec in (old,new):require_wildcard_target(spec)
 for key in ['templates','template_hits','rows','stride','benign_prefix','benign_suffix']:
  assert old[key]==new[key],key
 assert old['watchlist']['mv']==new['watchlist']['mv']
 assert [(p['id'],p['regex'],p['view']) for p in old['patterns']]==[(p['id'],p['regex'],p['view']) for p in new['patterns']]
 old['rows']=new['rows']=a.rows
 out=a.out/family;out.mkdir(exist_ok=False);engine=None
 marker=Path('.local/perf/execution/timing-active')
 if marker.exists():raise RuntimeError('another timing owner')
 marker.write_text('anchored watchlist '+family+'\n')
 try:
  engine=sweeps.Engine('.local/perf/antalya.json',timeout_seconds=a.timeout_seconds)
  native_setup(engine,old,out,False)
  engine.query(old['watchlist']['ddl'])
  engine.query('CREATE TABLE ingest_src(id Int64,v String) ENGINE=MergeTree ORDER BY id')
  engine.query('CREATE TABLE ingest_hits(id Int64,indicator_id Int64,view String) ENGINE=MergeTree ORDER BY id')
  engine.query(new['watchlist']['mv'])
  expected=expected_pairs(new);records=[]
  canary=[(row,int(h[0]),str(h[1])) for row,hits in enumerate(new['template_hits']) for h in hits]
  for spec in (old,new):
   engine.query('TRUNCATE TABLE watch');sweeps.inserts(engine,spec['watchlist']['inserts'])
   engine.query('TRUNCATE TABLE ingest_src');engine.query('TRUNCATE TABLE ingest_hits')
   engine.query('INSERT INTO ingest_src SELECT toInt64(id),v FROM templates')
   verify_pairs([(int(r[0]),int(r[1]),str(r[2])) for r in engine.query('SELECT id,indicator_id,view FROM ingest_hits')],canary)
  (out/'specs.json').write_text(json.dumps(dict(baseline=old,interior_grams=new),sort_keys=True)+'\n')
  (out/'settings.json').write_text(json.dumps(dict(engine=engine.query('SELECT version()')[0][0],settings=engine.settings,query_timeout_seconds=a.timeout_seconds,machine=sweeps.machine_settings(),timer='native HTTP insertion into real MV; complete hit pairs checked after clock; ranking/setup outside timer',variants='only required-needle metadata differs; regex/view bindings and MV SQL identical'),indent=2)+'\n')
  for warmup,repeat,label in schedule(['baseline','interior_grams'],a.repeats):
   spec=old if label=='baseline' else new
   engine.query('TRUNCATE TABLE watch')
   sweeps.inserts(engine,spec['watchlist']['inserts'])
   engine.query('TRUNCATE TABLE ingest_src');engine.query('TRUNCATE TABLE ingest_hits')
   qid=engine.prefix+uuid.uuid4().hex
   pending=dict(strategy=label,warmup=warmup,repeat=repeat,query_id=qid,query_timeout_seconds=a.timeout_seconds)
   (out/'pending.json').write_text(json.dumps(pending)+'\n');start=time.monotonic()
   try:engine.query('INSERT INTO ingest_src SELECT id,v FROM bench',query_id=qid)
   except Exception as error:
    (out/'failure.json').write_text(json.dumps(dict(**pending,elapsed_seconds=time.monotonic()-start,error=str(error),completed_sample=False),indent=2)+'\n')
    raise
   elapsed=time.monotonic()-start
   pairs=[(int(r[0]),int(r[1]),str(r[2])) for r in engine.query('SELECT id,indicator_id,view FROM ingest_hits')]
   digest=verify_pairs(pairs,expected)
   record=dict(strategy=label,warmup=warmup,repeat=repeat,request_ms=elapsed*1000,pairs=len(pairs),pair_sha256=digest,query_id=qid,profile=engine.profile(qid),host_load=list(os.getloadavg()))
   records.append(record);(out/'pending.json').unlink()
   with (out/'raw.jsonl').open('a') as f:f.write(json.dumps(record)+'\n')
   print(family,label,repeat,warmup,round(elapsed,3),flush=True)
  summary={label:dict(samples=len(xs),median_ms=statistics.median(xs),min_ms=min(xs),max_ms=max(xs),indicators_with_needles=sum(bool(p['needles']) for p in spec['patterns'])) for label,spec in [('baseline',old),('interior_grams',new)] for xs in [[r['request_ms'] for r in records if r['strategy']==label and not r['warmup']]]}
  candidate_counts={}
  for label,spec in [('baseline',old),('interior_grams',new)]:
   expressions=[]
   for pattern in spec['patterns']:
    condition=' AND '.join('position(v,'+sweeps.literal(n)+')>0' for n in pattern['needles']) or '1'
    expressions.append('sum(toUInt64('+condition+'))')
   candidates=0
   for start in range(0,len(expressions),32):
    candidates+=sum(int(n) for n in engine.query('SELECT '+','.join(expressions[start:start+32])+' FROM bench')[0])
   candidate_counts[label]=dict(candidate_pairs=candidates,total_pairs=a.rows*spec['indicators'],fraction=candidates/(a.rows*spec['indicators']))
  (out/'candidate-counts.json').write_text(json.dumps(dict(counts=candidate_counts,meaning='logical necessary-substring passes, not an instrumented regex invocation count; collected outside insertion timers'),indent=2)+'\n')
  (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');status.append(dict(family=family,status='PASS',summary=summary))
 finally:
  if engine:engine.close()
  marker.unlink(missing_ok=True)
  (a.out/'status.json').write_text(json.dumps(status,indent=2)+'\n')
