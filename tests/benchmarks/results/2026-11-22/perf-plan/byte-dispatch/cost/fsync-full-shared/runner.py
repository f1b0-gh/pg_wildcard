"""Bounded-cost control using the qualified public indexed ingestion helpers."""
import argparse,hashlib,json,signal,sys,uuid
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
import indexed_ingest
from compare_routes import sweeps,emit,storage,source_hash
from comparison import corpus_digest
from elasticsearch import Client
p=argparse.ArgumentParser();p.add_argument('--spec',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args()
marker=Path('.local/perf/execution/timing-active')
if marker.exists():raise RuntimeError('another measurement is active')
a.out.mkdir(exist_ok=False)
spec=json.loads(a.spec.read_text())
indexed_ingest.select_spec(spec)
for name in ('indexed_ingest.py','compare_routes.py','comparison.py','elasticsearch.py'):
 (a.out/name).write_bytes((Path('tests/benchmarks')/name).read_bytes())
(a.out/'runner.py').write_bytes(Path(__file__).read_bytes())
(a.out/'spec.json').write_text(json.dumps(spec,sort_keys=True)+'\n')
(a.out/'settings.json').write_text(json.dumps(dict(contract='configured fsync acknowledgment: native inserted files and part directory; Elastic request translog. No hardware/crash test claim.',batch_documents=2000,rows=spec['rows'],corpus_sha256=corpus_digest(spec),operation_order=['native','elastic'],timer='HTTP upload and index maintenance through count-confirmed visibility; preparation and compaction separate',resources=json.loads(Path('.local/perf/byte-dispatch/fair/resources.json').read_text()),source_sha256=hashlib.sha256(Path('tests/benchmarks/indexed_ingest.py').read_bytes()).hexdigest(),machine=sweeps.machine_settings()),indent=2)+'\n')
engine=None;client=None;index=None
with marker.open('x') as f:f.write('matched fsync ingestion cost control\n')
def interrupted(*_):raise KeyboardInterrupt('fsync control interrupted')
signal.signal(signal.SIGTERM,interrupted)
try:
 engine=sweeps.Engine('.local/perf/antalya.json',timeout_seconds=60)
 indexed_ingest.native_setup(engine,spec,a.out,emit=emit,storage=storage,source_hash=source_hash,literal=sweeps.literal,durability='fsync')
 client=Client(json.loads(Path('.local/perf/byte-dispatch/fair/elasticsearch.private.json').read_text()),70)
 index='wc_fsync_'+uuid.uuid4().hex
 indexed_ingest.elastic_setup(client,index,'wildcard',spec,a.out,emit=emit,durability='fsync')
 print('FSYNC CONTROL COMPLETE',flush=True)
except BaseException as error:
 (a.out/'failure.json').write_text(json.dumps(dict(type=type(error).__name__,message='inspect private log; incomplete cost control'))+'\n')
 raise
finally:
 errors=[]
 if client and index:
  try:client.request('DELETE','/'+index)
  except Exception as error:errors.append(type(error).__name__)
 if engine:
  try:engine.close()
  except Exception as error:errors.append(type(error).__name__)
 (a.out/'cleanup.json').write_text(json.dumps({'failures':errors})+'\n');marker.unlink(missing_ok=True)
 if errors:raise RuntimeError('owned control cleanup failed; stop further measurements')
