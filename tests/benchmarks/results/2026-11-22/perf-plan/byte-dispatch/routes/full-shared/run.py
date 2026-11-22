"""Private experiment adapter: shared indexed source for native/direct/PG and one Elastic index."""
import hashlib,json,os,signal,sys
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
sys.path.insert(0,str(Path(__file__).resolve().parent))
import wire_setup
root=Path.cwd();assert (root/'tests/benchmarks/compare_routes.py').is_file()
marker=root/'.local/perf/execution/timing-active'
assert not marker.exists()
args=sys.argv[1:];spec_path=Path(args[0]);out=Path(args[args.index('--out')+1])
assert not out.exists()
spec=json.loads(spec_path.read_text());assert spec['byte_dispatch']['fallback'] is None
spec['queries']=spec['byte_dispatch']['queries'];spec['diagnostics']=dict(strategy='byte_dispatch',source_queries=1,estimated_ast=spec['byte_dispatch']['estimated_ast'])
adapted=out.parent/(out.name+'-spec.json');adapted.parent.mkdir(parents=True,exist_ok=True)
assert not adapted.exists();adapted.write_text(json.dumps(spec,sort_keys=True)+'\n');args[0]=str(adapted)
source=(root/'tests/benchmarks/compare_routes.py').read_text()
old="ROUTES=('native','indexed','direct','pg_backend','pg_copy','elastic_wildcard','elastic_keyword')"
assert source.count(old)==1;source=source.replace(old,old[:-1]+",'elastic_wildcard_4')")
old="if route.startswith('elastic_'):return elastic_run(elastic,indexes[route[8:]],spec,args.elastic_searches)"
new="if route.startswith('elastic_'):return elastic_run(elastic,indexes['wildcard' if route=='elastic_wildcard_4' else route[8:]],spec,4 if route=='elastic_wildcard_4' else args.elastic_searches)"
assert source.count(old)==1;source=source.replace(old,new)
namespace={'__file__':str(root/'tests/benchmarks/compare_routes.py'),'__name__':'private_fair_routes'}
exec(compile(source,namespace['__file__'],'exec'),namespace)
namespace.update(native_setup=wire_setup.native_setup,elastic_setup=wire_setup.elastic_setup)
sys.argv=['private-fair-routes',*args]
with marker.open('x') as f:f.write('byte dispatch matched indexed routes\n')
def interrupted(*_):raise KeyboardInterrupt('owned comparison interrupted')
signal.signal(signal.SIGTERM,interrupted)
try:
 namespace['main']()
 (out/'executed-collector.py').write_text(source)
 for name in ('run.py','wire_setup.py'):(out/name).write_bytes((Path(__file__).parent/name).read_bytes())
 settings=json.loads((out/'settings.json').read_text())
 settings.update(query_layout='bounded hex-byte-fragment text index for every native/direct/PG route',elastic_route_concurrency={'elastic_wildcard':1,'elastic_wildcard_4':4},wire_contract='same raw documents,2000-document HTTP batches,90% initial/10% incremental; upload through count-confirmed visibility; serialized payload prep and compaction separately recorded',wrapper_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),executed_collector_sha256=hashlib.sha256(source.encode()).hexdigest())
 (out/'settings.json').write_text(json.dumps(settings,indent=2)+'\n')
 (out/'cleanup.json').write_text('{"failures":[]}\n')
except BaseException as error:
 if out.exists():(out/'failure.json').write_text(json.dumps({'type':type(error).__name__,'message':'inspect private log; no timing claim'})+'\n')
 raise
finally:marker.unlink(missing_ok=True)
