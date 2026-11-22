"""Matched indexed-table sweep experiment; all results retain full pair checks."""
import argparse,gzip,hashlib,json,os,signal,statistics,sys,time
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from compare_routes import sweeps,native_setup,native_run,storage,source_hash
from comparison import expected_pairs,schedule,verify_pairs,corpus_digest
p=argparse.ArgumentParser();p.add_argument('--spec-dir',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
p.add_argument('--rows',type=int,choices=(10000,1000000),required=True);p.add_argument('--repeats',type=int,choices=(1,5),required=True)
p.add_argument('--coarse-spec-dir',type=Path,required=True);p.add_argument('--families',default='shared,unrelated');a=p.parse_args()
families=a.families.split(',');assert families and len(set(families))==len(families) and set(families)<= {'shared','unrelated'}
a.out.mkdir(exist_ok=False)
(a.out/'runner.py').write_bytes(Path(__file__).read_bytes())
marker=Path('.local/perf/execution/timing-active')
def interrupted(*_):raise KeyboardInterrupt('bounded indexed sweep experiment interrupted')
signal.signal(signal.SIGTERM,interrupted)
for family in families:
 out=a.out/family;out.mkdir();engine=None
 spec=json.loads((a.spec_dir/(family+'.json')).read_text());assert spec['rows']==a.rows and spec['indicators']==1000
 (out/'spec.json.gz').write_bytes(gzip.compress(json.dumps(spec,sort_keys=True).encode(),mtime=0))
 expected=expected_pairs(spec)
 with marker.open('x') as f:f.write('bounded byte dispatch '+family+'\n')
 try:
  engine=sweeps.Engine('.local/perf/antalya.json',timeout_seconds=60)
  assert native_setup(engine,spec,out,True)
  dispatch=spec['byte_dispatch']
  assert dispatch['strategy']=='byte_dispatch' and dispatch['fallback'] is None
  engine.query('CREATE TABLE bench_bytes AS bench')
  split=a.rows*9//10
  engine.query(f'INSERT INTO bench_bytes SELECT * FROM bench WHERE id < {split}')
  start=time.monotonic()
  ddl=dispatch['index_ddl'].replace('TABLE bench ', 'TABLE bench_bytes ')
  engine.query(ddl)
  engine.query('ALTER TABLE bench_bytes MATERIALIZE INDEX v_byte_grams SETTINGS mutations_sync=2')
  build_seconds=time.monotonic()-start
  start=time.monotonic()
  engine.query(f'INSERT INTO bench_bytes SELECT * FROM bench WHERE id >= {split}')
  insert_seconds=time.monotonic()-start
  engine.query('OPTIMIZE TABLE bench_bytes FINAL')
  assert source_hash(engine,'bench_bytes')==corpus_digest(spec)
  (out/'byte-index-build.json').write_text(json.dumps(dict(ddl=ddl,build_seconds=build_seconds,
    incremental_insert_seconds=insert_seconds,storage=storage(engine,'bench_bytes'),
    scope='native server-to-server copy and index work; not an end-to-end Elastic ingestion comparison'),indent=2)+'\n')
  variants={'constant_candidates':[q.replace('FROM bench\n','FROM '+engine.database+'.bench_indexed\n') for q in spec['indexed_queries']],
    'byte_dispatch':[q.replace('FROM bench\n','FROM '+engine.database+'.bench_bytes\n') for q in dispatch['queries']]}
  coarse=json.loads((a.coarse_spec_dir/(family+'.json')).read_text())
  assert all(coarse[key]==spec[key] for key in ('rows','indicators','patterns','templates','template_hits','stride','pattern_sha256'))
  variants['byte_dispatch_coarse']=[q.replace('FROM bench\n','FROM '+engine.database+'.bench_bytes\n') for q in coarse['byte_dispatch']['queries']]
  (out/'coarse-spec.json.gz').write_bytes(gzip.compress(json.dumps(coarse,sort_keys=True).encode(),mtime=0))
  (out/'variants.json.gz').write_bytes(gzip.compress(json.dumps(variants).encode(),mtime=0))
  (out/'settings.json').write_text(json.dumps(dict(engine=engine.query('SELECT version()')[0][0],settings=engine.settings,
   machine=sweeps.machine_settings(),resources={k:engine.state[k] for k in ('cpus','memory_bytes') if k in engine.state},
   rows=a.rows,indicators=1000,repeats=a.repeats,timer='native HTTP through complete hit-pair consumption; validation/profiles outside timer',
   source_sha256=hashlib.sha256(Path('libwildcard/src/antalya/indexed_dispatch.rs').read_bytes()).hexdigest()),indent=2)+'\n')
  for label,queries in variants.items():
   canary_queries=[q.replace(engine.database+'.bench_indexed',engine.database+'.templates').replace(engine.database+'.bench_bytes',engine.database+'.templates') for q in queries]
   canary=[(row,int(hit[0]),str(hit[1])) for row,hits in enumerate(spec['template_hits']) for hit in hits]
   verify_pairs([(int(r[0]),int(r[1]),str(r[2])) for r in native_run(engine,canary_queries)['pairs']],canary)
   for command in ('EXPLAIN indexes=1 ','EXPLAIN PLAN actions=1 '):
    result=engine.query(command+queries[0],logging=False)
    (out/(label+('-indexes' if 'indexes' in command else '-actions')+'.json.gz')).write_bytes(gzip.compress(json.dumps(result).encode(),mtime=0))
  source_gates={}
  for label,queries in variants.items():
   if not label.startswith('byte_dispatch'):continue
   source=queries[0].split('\nFROM (',1)[1].rsplit('\n)\nARRAY JOIN ',1)[0]
   source_gates[label]=dict(candidate_rows=int(engine.query('SELECT count() FROM ('+source+") SETTINGS force_data_skipping_indices='v_byte_grams'")[0][0]),source_rows=a.rows,forced_index=True)
  (out/'source-gates.json').write_text(json.dumps(source_gates,indent=2)+'\n')
  records=[]
  for warmup,repeat,label in schedule(list(variants),a.repeats):
   pending=dict(strategy=label,warmup=warmup,repeat=repeat)
   (out/'pending.json').write_text(json.dumps(pending)+'\n')
   record=native_run(engine,variants[label]);pairs=[(int(r[0]),int(r[1]),str(r[2])) for r in record.pop('pairs')];digest=verify_pairs(pairs,expected)
   record.update(**pending,pairs=len(pairs),pair_sha256=digest,profiles=[engine.profile(q) for q in record['query_ids']],host_load=list(os.getloadavg()))
   record['source_references']=len(variants[label])
   records.append(record)
   with (out/'raw.jsonl').open('a') as f:f.write(json.dumps(record)+'\n')
   (out/'pending.json').unlink()
   print(family,label,warmup,repeat,round(record['request_ms'],3),sum(x['read_rows'] for x in record['profiles']),flush=True)
  summary={}
  for label in variants:
   selected=[r for r in records if r['strategy']==label and not r['warmup']]
   values=[r['request_ms'] for r in selected]
   summary[label]=dict(samples=len(values),median_ms=statistics.median(values),min_ms=min(values),max_ms=max(values),
     read_rows=[sum(p['read_rows'] for p in r['profiles']) for r in selected],read_bytes=[sum(p['read_bytes'] for p in r['profiles']) for r in selected])
  (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
 except BaseException as error:
  (out/'failure.json').write_text(json.dumps(dict(type=type(error).__name__,error=sweeps.redact(str(error),engine.state if engine else {})))+'\n')
  raise
 finally:
  try:
   if engine:engine.close()
   (out/'cleanup.json').write_text(json.dumps({'failures':[]})+'\n')
  finally:marker.unlink(missing_ok=True)
