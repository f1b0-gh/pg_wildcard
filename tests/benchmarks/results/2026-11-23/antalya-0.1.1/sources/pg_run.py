#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Private PG rerun against the same original sweep specs; requires the recorded patch."""
import gzip,hashlib,json,os,statistics,sys,uuid
from pathlib import Path
from run import ROOT,BASE,write,sweeps,storage,source_hash,emit,native_setup,select_spec,expected_pairs,verify_pairs,corpus_digest
from compare_routes import foreign_nodes
from fdw_profile import pg_literal
from route_workers import PgWorker
CONTAINER=os.environ['WILDCARD_BENCH_PG_CONTAINER']
VARIANTS=[('backend-none','none',True),('backend-zstd','zstd',True),('copy-none','none',False),('copy-zstd','zstd',False)]
def run():
 state=BASE/'antalya.private.json';private=json.loads(state.read_text());out=BASE/'postgres';out.mkdir(exist_ok=False)
 write(out/'settings.json',dict(runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),machine=sweeps.machine_settings(),client='published antalya-rs 0.1.1',extension='8ae6be1 + recorded benchmark-only compression option patch',engine_image=private['image'],engine_resources={k:private[k] for k in ('cpus','memory_bytes')},repeats=6,warmups=1,terminal='flush_first',max_batch_bytes=16<<20,retained_bytes=64<<20,order='base order then reversed each round',backend_clock='EXPLAIN ANALYZE Execution Time',copy_clock='persistent psql request through complete binary COPY file, excludes decode/hash',query_log='correlated after each sample outside clock; one fresh QueryFinish required'))
 engine=None;admin=None;workers={};schemas=[];records=[]
 try:
  admin=PgWorker(CONTAINER,ROOT,out/'admin.private.log')
  admin.execute('SELECT version();');write(out/'postgres-version.json',admin.file.read_text().strip())
  for workload in ('shared','unrelated'):
   case=out/workload;case.mkdir()
   original=gzip.decompress((ROOT/'tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/routes'/('full-'+workload)/'spec.json.gz').read_bytes());spec=select_spec(json.loads(original))
   (case/'spec.json').write_bytes(original);expected=expected_pairs(spec)
   engine=sweeps.Engine(state,timeout_seconds=60)
   native_setup(engine,spec,case,emit=emit,storage=storage,source_hash=source_hash,literal=sweeps.literal,durability='defaults')
   queries=[q.replace('FROM bench\n','FROM '+engine.database+'.bench\n') for q in spec['queries']];assert len(queries)==1
   engine.query('CREATE TABLE canary AS bench');engine.query('INSERT INTO canary SELECT toInt64(id),v FROM templates')
   canary=queries[0].replace(engine.database+'.bench',engine.database+'.canary')
   oracle=[(row,int(hit[0]),str(hit[1])) for row,hits in enumerate(spec['template_hits']) for hit in hits]
   sqls={};schema_ids={}
   for codec in ('none','zstd'):
    schema='wc_codec_'+uuid.uuid4().hex;schemas.append(schema);schema_ids[codec]=schema
    ddl=f'CREATE SCHEMA {schema}; CREATE SERVER {schema} FOREIGN DATA WRAPPER wildcard_fdw OPTIONS(url {pg_literal(private["url"])},database {pg_literal(engine.database)},timeout_ms \'65000\',settings \'max_threads=4,max_block_size=8192,max_execution_time=60\',terminal_policy \'flush_first\',arrow_compression {pg_literal(codec)}); CREATE USER MAPPING FOR CURRENT_USER SERVER {schema} OPTIONS(bearer_token {pg_literal(private["token"])});'
    admin.execute(ddl)
    for suffix,q in [('bench',queries[0]),('canary',canary)]:
     admin.execute(f'CREATE FOREIGN TABLE {schema}.{suffix}(id bigint,indicator_id bigint,view text) SERVER {schema} OPTIONS(table {pg_literal("("+q+") AS sweep_pairs")});')
    workers[codec]=PgWorker(CONTAINER,ROOT,case/(codec+'.private.log'))
    sqls[codec]='SELECT id,indicator_id,view FROM '+schema+'.bench'
    for suffix,truth in [('canary',oracle),('bench',expected)]:
     r=workers[codec].request('SELECT id,indicator_id,view FROM '+schema+'.'+suffix)
     emit(case,kind='precontrol',codec=codec,target=suffix,pair_sha256=verify_pairs(r['pairs'],truth),pairs=len(r['pairs']))
   write(case/'queries.json',dict(native=queries,postgres=sqls))
   write(case/'native-explain.json',[engine.query('EXPLAIN indexes=1 '+q) for q in queries])
   write(case/'identity.json',dict(rows=spec['rows'],indicators=spec['indicators'],corpus_sha256=corpus_digest(spec),pattern_sha256=spec['pattern_sha256'],spec_sha256=hashlib.sha256(original).hexdigest()))
   def finishes():
    engine.query('SYSTEM FLUSH LOGS',logging=False)
    return {r[0] for r in engine.query("SELECT query_id FROM system.query_log WHERE type='QueryFinish' AND startsWith(query_id,'wildcard-') AND position(query,"+sweeps.literal(engine.database+'.bench')+")>0",logging=False)}
   seen=finishes()
   for repeat in range(-1,6):
    order=VARIANTS if repeat<0 or repeat%2==0 else list(reversed(VARIANTS))
    for position,(label,codec,backend) in enumerate(order):
     load=os.getloadavg();r=workers[codec].request(sqls[codec],backend=backend)
     if backend:
      nodes=list(foreign_nodes(r['plan']['Plan']));assert len(nodes)==1
      assert sum(n['Actual Rows']*n['Actual Loops'] for n in nodes)==len(expected)
      r['query_ids']=[n['FDW last query ID'] for n in nodes]
      r['verification']='exact full COPY precontrol on same SQL; backend checks actual rows'
     else:
      pairs=r.pop('pairs');r['pairs']=len(pairs);r['pair_sha256']=verify_pairs(pairs,expected)
     record=dict(kind='sample',workload=workload,variant=label,repeat=repeat,warmup=repeat<0,position=position,host_load=load,**r)
     # Keep clock/result even if subsequent terminal correlation fails.
     emit(case,**record);records.append(record)
     now=finishes();fresh=now-seen;seen=now
     assert len(fresh)==1, ('unexpected QueryFinish count',len(fresh))
     qid=fresh.pop()
     if backend:assert r['query_ids']==[qid]
     profile=engine.profile(qid)
     assert profile['settings'].get('output_format_arrow_compression_method')==codec
     assert int(profile['result_rows'])==10000
     emit(case,kind='profile',variant=label,repeat=repeat,query_id=qid,profile=profile)
   for w in workers.values():w.close()
   workers={}
   for schema in schemas:admin.execute('DROP SCHEMA '+schema+' CASCADE; DROP SERVER '+schema+' CASCADE;')
   schemas=[];engine.close();engine=None
  summary={}
  for workload in ('shared','unrelated'):
   summary[workload]={}
   for label,_,backend in VARIANTS:
    rs=[r for r in records if r['workload']==workload and r['variant']==label and not r['warmup']]
    key='executor_ms' if backend else 'request_ms';vs=[r[key] for r in rs]
    summary[workload][label]=dict(median_ms=statistics.median(vs),samples_ms=vs,clock=key)
  write(out/'summary.json',summary);print(json.dumps(summary,indent=2))
 finally:
  for w in workers.values():w.close()
  if admin:
   for schema in schemas:admin.execute('DROP SCHEMA IF EXISTS '+schema+' CASCADE; DROP SERVER IF EXISTS '+schema+' CASCADE;')
   admin.close()
  if engine is not None:engine.close()
if __name__=='__main__':run()
