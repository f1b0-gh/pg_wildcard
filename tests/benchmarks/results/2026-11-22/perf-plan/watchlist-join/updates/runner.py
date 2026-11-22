"""Native watchlist upsert cost and first subsequent insert; no regex engine change."""
import hashlib,json,os,re,signal,statistics,sys,time,uuid
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from compare_routes import sweeps,native_setup
from comparison import expected_pairs,require_wildcard_target,schedule,verify_pairs

root=Path('.local/perf/percolator/update-cost')
root.mkdir(exist_ok=False)
(root/'runner.py').write_bytes(Path(__file__).read_bytes())
marker=Path('.local/perf/execution/timing-active')

def terminate(_signal,_frame):raise KeyboardInterrupt('owned watchlist update benchmark termination')
signal.signal(signal.SIGTERM,terminate)

for family,count,block in [('domains',100,1024),('domains',100,8192),('shared',1000,1024),('shared',1000,8192)]:
 if marker.exists():raise RuntimeError('another timing window is active')
 marker.write_text(f'watchlist update cost {family} {count} block={block}\n')
 out=root/f'{family}-{count}-b{block}';out.mkdir();engine=None;completed=False
 try:
  spec=json.loads(Path(f'.local/perf/percolator/production-specs/{family}-{count}-1000000.json').read_text())
  require_wildcard_target(spec);spec['rows']=10000
  assert len(spec['patterns'])==count and [p['id'] for p in spec['patterns']]==list(range(count))
  plan=spec['watchlist']['join_plan'];assert plan['strategy']=='rule_anchor_join' and plan['fallback'] is None
  (out/'spec.json').write_text(json.dumps(spec,sort_keys=True)+'\n')
  engine=sweeps.Engine('.local/perf/antalya.json',timeout_seconds=120)
  engine.settings.update(max_block_size=block,max_insert_block_size=block,min_insert_block_size_rows=block)
  native_setup(engine,spec,out,False)
  engine.query(spec['watchlist']['ddl']);sweeps.inserts(engine,spec['watchlist']['inserts'])
  engine.query('CREATE TABLE ingest_src(id Int64,v String) ENGINE=MergeTree ORDER BY id')
  engine.query('CREATE TABLE ingest_hits(id Int64,indicator_id Int64,view String) ENGINE=MergeTree ORDER BY id')
  variants={'staged_needles':spec['watchlist']['mv'],'production_join':plan['sql']}
  names=[]
  for sql in variants.values():
   m=re.match(r'CREATE MATERIALIZED VIEW (?:IF NOT EXISTS )?([A-Za-z_]\w*) ',sql)
   if not m:raise ValueError('unexpected generated MV name')
   names.append(m[1])
  expected=expected_pairs(spec)
  def pairs():return [tuple(r) for r in engine.query('SELECT id,indicator_id,view FROM ingest_hits')]
  def install(strategy):
   for name in names:engine.query('DROP VIEW IF EXISTS '+name)
   engine.query(variants[strategy]);engine.query('TRUNCATE TABLE ingest_src');engine.query('TRUNCATE TABLE ingest_hits')
  # Physical deletion, not only active=0: absence must affect the next block.
  install('production_join')
  engine.query(f'ALTER TABLE watch DELETE WHERE id={count-1} SETTINGS mutations_sync=1')
  engine.query('INSERT INTO ingest_src SELECT toInt64(id),v FROM templates')
  canary=[(row,int(hit[0]),str(hit[1])) for row,hits in enumerate(spec['template_hits']) for hit in hits]
  verify_pairs(pairs(),[p for p in canary if p[1]!=count-1])
  sweeps.inserts(engine,spec['watchlist']['inserts'])
  engine.query('TRUNCATE TABLE ingest_hits')
  engine.query('INSERT INTO ingest_src SELECT toInt64(id),v FROM templates')
  verify_pairs(pairs(),canary)
  (out/'delete-reinsert-control.json').write_text(json.dumps({'deleted_id':count-1,'before_pairs':len(canary),'after_delete_pairs':len([p for p in canary if p[1]!=count-1]),'restored_pairs':len(canary),'status':'PASS'})+'\n')
  metadata={'engine':engine.query('SELECT version()')[0][0],'settings':engine.settings,'machine':sweeps.machine_settings(),
   'rows':10000,'indicators':count,'requested_block_rows':block,'repeats':5,'batch_rules':128,
   'update_operation':'bulk upsert all stable rule IDs with a rotated assignment of existing regex/view/needle tuples; global pattern set unchanged; fresh initial watch snapshot per trial',
   'update_clock':'HTTP request through completed native INSERTs in batches of 128; pre-generated SQL, no PostgreSQL or client regex compilation',
   'insert_clock':'separate first resident-source INSERT SELECT after update validation, including source and hit persistence',
   'boundaries':'oracle, FINAL visibility validation, truncation, query-log reads and checksum are outside both clocks; clocks are not one end-to-end request',
   'runner_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
  (out/'settings.json').write_text(json.dumps(metadata,indent=2)+'\n')
  records=[]
  for sequence,(warmup,repeat,strategy) in enumerate(schedule(list(variants),5),1):
   install(strategy);shift=(1 if warmup else repeat+2)%count
   engine.query('TRUNCATE TABLE watch');sweeps.inserts(engine,spec['watchlist']['inserts'])
   statements=[]
   for original,statement in enumerate(spec['watchlist']['inserts']):
    prefix,value=statement.split(' VALUES ',1)
    match=re.match(r'^\((\d+),',value)
    if not match or int(match[1])!=original:raise ValueError('fixture insert ID mismatch')
    statements.append(prefix+' VALUES ('+str((original+shift)%count)+value[match.end()-1:])
   prefix=statements[0].split(' VALUES ',1)[0]+' VALUES '
   queries=[prefix+', '.join(s.split(' VALUES ',1)[1] for s in statements[start:start+128]) for start in range(0,count,128)]
   if max(len(q.encode()) for q in queries)>200000:raise ValueError('update SQL exceeds fixture budget')
   ids=[engine.prefix+uuid.uuid4().hex for _ in queries]
   (out/f'update-{sequence}.json').write_text(json.dumps({'strategy':strategy,'queries':queries,'query_ids':ids},indent=2)+'\n')
   pending=dict(strategy=strategy,warmup=warmup,repeat=repeat,sequence=sequence,shift=shift)
   (out/'pending.json').write_text(json.dumps(pending)+'\n')
   start=time.monotonic()
   for sql,qid in zip(queries,ids):engine.query(sql,query_id=qid)
   update_ms=(time.monotonic()-start)*1000
   actual=sorted(tuple(r) for r in engine.query('SELECT id,regex,view FROM watch FINAL WHERE active=1'))
   wanted=sorted(((p['id']+shift)%count,p['regex'],p['view']) for p in spec['patterns'])
   if actual!=wanted:raise ValueError('upsert visibility or version ordering mismatch')
   qid=engine.prefix+uuid.uuid4().hex;start=time.monotonic()
   engine.query('INSERT INTO ingest_src SELECT id,v FROM bench',query_id=qid)
   insert_ms=(time.monotonic()-start)*1000
   wanted_pairs=[(row,(indicator+shift)%count,view) for row,indicator,view in expected]
   digest=verify_pairs(pairs(),wanted_pairs)
   record=dict(**pending,update_ms=update_ms,post_update_insert_ms=insert_ms,update_query_ids=ids,
    insert_query_id=qid,update_profiles=[engine.profile(i) for i in ids],insert_profile=engine.profile(qid),
    pairs=len(wanted_pairs),pair_sha256=digest,host_load=list(os.getloadavg()))
   with (out/'updates.jsonl').open('a') as f:f.write(json.dumps(record)+'\n')
   records.append(record);(out/'pending.json').unlink()
   print(family,count,block,strategy,warmup,repeat,round(update_ms,2),round(insert_ms,2),flush=True)
  summary={}
  for strategy in variants:
   measured=[r for r in records if r['strategy']==strategy and not r['warmup']]
   assert len(measured)==5
   summary[strategy]={}
   for metric in ('update_ms','post_update_insert_ms'):
    values=[r[metric] for r in measured]
    summary[strategy][metric]=dict(samples=5,median=statistics.median(values),min=min(values),max=max(values))
  (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');completed=True
 finally:
  try:
   if engine:engine.close()
   (out/'cleanup.json').write_text(json.dumps({'completed':completed,'owned_database_removed':engine is not None})+'\n')
  finally:marker.unlink(missing_ok=True)
(root/'completion.json').write_text(json.dumps({'exit_status':0,'cases':4})+'\n')
