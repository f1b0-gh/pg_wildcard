#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Matched published-release comparison. All query timing excludes oracle hashing."""
import gzip,hashlib,json,os,re,statistics,sys,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT/'tests/benchmarks'))
from compare_routes import sweeps,storage,source_hash,emit
from indexed_ingest import native_setup,select_spec
from comparison import expected_pairs,verify_pairs,require_wildcard_target,corpus_digest
from route_workers import DirectWorker
BASE=Path(__file__).resolve().parent
TARGET=ROOT/'.local/review-pg/target/release'
VARIANTS=[('010-none','0.1.0','none'),('011-none','0.1.1','none'),('011-zstd','0.1.1','zstd')]
def write(path,value):path.write_text(json.dumps(value,indent=2)+'\n')
def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def run():
 state=BASE/'antalya.private.json';private=json.loads(state.read_text())
 out=BASE/'direct';out.mkdir(exist_ok=False)
 write(out/'settings.json',dict(machine=sweeps.machine_settings(),engine_image=private['image'],engine_resources={k:private[k] for k in ('cpus','memory_bytes')},variants=VARIANTS,repeats=6,warmups=1,terminal='flush_first',max_batch_bytes=16<<20,retained_bytes=64<<20,query_settings=sweeps.SETTINGS,cache='query and condition caches off; OS caches uncontrolled',clock='worker request_ms: fetch, decode, consume values and terminal confirmation; excludes stdout serialization and Python oracle',order='rotate by round; six repetitions balance three positions',sources={str(p.relative_to(ROOT)):digest(p) for p in [Path(__file__),ROOT/'tests/transport/examples/sweep_route.rs',ROOT/'tests/benchmarks/indexed_ingest.py',ROOT/'tests/benchmarks/comparison.py',ROOT/'tests/benchmarks/route_workers.py',ROOT/'libwildcard/tests/benchmarks/sweeps.py']},binaries={label:digest(TARGET/('sweep-release-'+version.replace('.',''))) for label,version,_ in VARIANTS}))
 workers={};engine=None;records=[]
 try:
  for label,version,codec in VARIANTS:
   workers[label]=DirectWorker(TARGET/('sweep-release-'+version.replace('.','')),private,out/(label+'.private.log'),[codec])
   assert workers[label].ready['client']==version and workers[label].ready['codec']==codec
   emit(out,kind='worker',variant=label,**workers[label].ready)
  for workload in ('shared','unrelated'):
   case=out/workload;case.mkdir()
   saved=ROOT/'tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/routes'/('full-'+workload)
   original=gzip.decompress((saved/'spec.json.gz').read_bytes());spec=select_spec(json.loads(original))
   (case/'spec.json').write_bytes(original);require_wildcard_target(spec)
   engine=sweeps.Engine(state,timeout_seconds=60)
   native_setup(engine,spec,case,emit=emit,storage=storage,source_hash=source_hash,literal=sweeps.literal,durability='defaults')
   queries=[q.replace('FROM bench\n','FROM '+engine.database+'.bench\n') for q in spec['queries']]
   historical=json.loads(gzip.decompress((saved/'queries.json.gz').read_bytes()))['native']
   normalize=lambda q:re.sub(r'wc_benchmark_[a-f0-9]+\.bench','bench',q)
   assert list(map(normalize,queries))==list(map(normalize,historical)), 'historical SQL differs'
   expected=expected_pairs(spec);assert len(expected)==10000
   write(case/'queries.json',queries)
   write(case/'native-explain.json',[engine.query('EXPLAIN indexes=1 '+q) for q in queries])
   write(case/'identity.json',dict(workload=workload,rows=spec['rows'],indicators=spec['indicators'],corpus_sha256=corpus_digest(spec),pattern_sha256=spec['pattern_sha256'],spec_sha256=hashlib.sha256(original).hexdigest(),engine=engine.query('SELECT version()')[0][0],database=engine.database))
   engine.query('CREATE TABLE canary AS bench');engine.query('INSERT INTO canary SELECT toInt64(id),v FROM templates')
   canary=[q.replace(engine.database+'.bench',engine.database+'.canary') for q in queries]
   oracle=[(row,int(hit[0]),str(hit[1])) for row,hits in enumerate(spec['template_hits']) for hit in hits]
   for label,_,_ in VARIANTS:
    r=workers[label].request(engine.database,canary)
    verified=verify_pairs([tuple(p) for p in r.pop('pairs')],oracle)
    emit(case,kind='canary',variant=label,pairs=len(oracle),pair_sha256=verified,**r)
   current=[]
   for repeat in range(-1,6):
    order=VARIANTS if repeat<0 else VARIANTS[repeat%3:]+VARIANTS[:repeat%3]
    for position,(label,_,codec) in enumerate(order):
     load=os.getloadavg();r=workers[label].request(engine.database,queries)
     pairs=r.pop('pairs');verified=verify_pairs([tuple(p) for p in pairs],expected)
     record=dict(kind='sample',workload=workload,variant=label,repeat=repeat,warmup=repeat<0,position=position,host_load=load,pairs=len(pairs),pair_sha256=verified,**r)
     emit(case,**record);current.append(record);records.append(record)
   # Administrative profiling stays outside the timed cohort.
   for r in current:
    for execution in r['executions']:
     profile=engine.profile(execution['query_id'])
     codec=next(v[2] for v in VARIANTS if v[0]==r['variant'])
     assert profile['settings'].get('output_format_arrow_compression_method')==codec,profile['settings']
     assert int(profile['result_rows'])==10000
     emit(case,kind='profile',variant=r['variant'],repeat=r['repeat'],query_id=execution['query_id'],profile=profile)
   engine.close();engine=None
  summary={}
  for workload in ('shared','unrelated'):
   summary[workload]={}
   for label,_,_ in VARIANTS:
    rs=[r for r in records if r['workload']==workload and r['variant']==label and not r['warmup']]
    summary[workload][label]={k:statistics.median(r[k] for r in rs) for k in ('request_ms','worker_roundtrip_ms')}
    summary[workload][label].update({k:statistics.median(sum(e[k] or 0 for e in r['executions']) for r in rs) for k in ('wire_bytes','decode_ms','body_wait_ms','terminal_ms')})
    summary[workload][label]['request_samples_ms']=[r['request_ms'] for r in rs]
  write(out/'summary.json',summary);print(json.dumps(summary,indent=2))
 finally:
  for worker in workers.values():worker.close()
  if engine is not None:engine.close()
if __name__=='__main__':run()
