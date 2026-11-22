"""Private paired insertion/storage cost of the bounded raw byte index."""
import json,sys,time,statistics,os
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from compare_routes import sweeps,native_setup,storage,source_hash
from comparison import corpus_digest
root=Path('.local/perf/indexed/write-cost');root.mkdir(exist_ok=False)
marker=Path('.local/perf/execution/timing-active')
if marker.exists():raise RuntimeError('another timing window')
marker.write_text('bounded byte index insertion costs\n')
expr="arrayMap(i -> if(i=8190,'_wildcard_overflow_',substring(v, i + 1, 3)), range(toUInt64(greatest(toInt64(least(length(v),8193)) - 2, 0))))"
engine=None
try:
 engine=sweeps.Engine('.local/perf/antalya.json',timeout_seconds=60)
 spec=json.loads(Path('.local/perf/specs/unrelated-1000.json').read_text());native_setup(engine,spec,root,False)
 records=[]
 for rep in range(5):
  for label in (['plain','byte_index'] if rep%2==0 else ['byte_index','plain']):
   table=f'{label}_{rep}';engine.query('CREATE TABLE '+table+' AS bench')
   ddl=None
   if label=='byte_index':
    ddl=f"ALTER TABLE {table} ADD INDEX grams {expr} TYPE text(tokenizer='array') GRANULARITY 1";engine.query(ddl)
   start=time.monotonic();engine.query(f'INSERT INTO {table} SELECT * FROM bench WHERE id<900000');initial=time.monotonic()-start
   start=time.monotonic();engine.query(f'INSERT INTO {table} SELECT * FROM bench WHERE id>=900000');incremental=time.monotonic()-start
   engine.query('OPTIMIZE TABLE '+table+' FINAL');actual=source_hash(engine,table)
   if actual!=corpus_digest(spec):raise AssertionError('full value digest mismatch')
   r=dict(strategy=label,repeat=rep,initial_900k_seconds=initial,incremental_100k_seconds=incremental,storage=storage(engine,table),corpus_sha256=actual,ddl=ddl,host_load=list(os.getloadavg()))
   records.append(r)
   with (root/'raw.jsonl').open('a') as f:f.write(json.dumps(r)+'\n')
   print(json.dumps(r),flush=True);engine.query('DROP TABLE '+table)
 summary={label:{key:dict(median=statistics.median(vals),min=min(vals),max=max(vals)) for key in ['initial_900k_seconds','incremental_100k_seconds'] for vals in [[r[key] for r in records if r['strategy']==label]]} for label in ['plain','byte_index']}
 (root/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
finally:
 if engine:engine.close()
 marker.unlink(missing_ok=True)
