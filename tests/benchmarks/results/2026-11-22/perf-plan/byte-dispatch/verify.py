# SPDX-License-Identifier: Apache-2.0
"""Verify this evidence bundle and reproduce reports without engine access."""
import gzip,hashlib,json,subprocess,sys,tempfile
from pathlib import Path
bundle=Path(__file__).resolve().parent
if sys.flags.optimize:raise RuntimeError('Run without -O: evidence reporters use assertions')
def require(condition,message):
 if not condition:raise ValueError(message)
def digest(data):return hashlib.sha256(data).hexdigest()
manifest=json.loads((bundle/'sha256.json').read_text())
actual={str(p.relative_to(bundle)) for p in bundle.rglob('*') if p.is_file()}
require(actual==set(manifest)|{'sha256.json'},'manifest membership differs')
for name,sha in manifest.items():
 rel=Path(name);require(not rel.is_absolute() and '..' not in rel.parts,'invalid payload path')
 raw=(bundle/name).read_bytes();require(digest(raw)==sha,'payload digest differs: '+name)
 if name.endswith('.gz'):gzip.decompress(raw)
layout=json.loads((bundle/'layout.json').read_text())
with tempfile.TemporaryDirectory(prefix='byte-dispatch-evidence-') as directory:
 root=Path(directory)
 for name,entry in layout.items():
  rel=Path(entry['restore']);require(not rel.is_absolute() and '..' not in rel.parts,'invalid restore path')
  raw=(bundle/name).read_bytes();raw=gzip.decompress(raw) if entry['gzip'] else raw
  require(digest(raw)==entry['source_sha256'],'restored source digest differs')
  target=root/rel;require(not target.exists(),'duplicate restore path')
  target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(raw)
 def run(script,arg):
  # Report imports may use stdlib networking modules, but cannot contact an engine
  # or spawn a helper process. The outer verifier alone starts these reporters.
  wrapper="import runpy,sys\ndef audit(event,args):\n if event in ('socket.connect','subprocess.Popen','os.system','os.posix_spawn','os.posix_spawnp'):raise RuntimeError('offline report attempted external action')\nsys.addaudithook(audit)\nsys.argv=sys.argv[1:]\nrunpy.run_path(sys.argv[0],run_name='__main__')\n"
  subprocess.run([sys.executable,'-B','-c',wrapper,script,arg],cwd=root,check=True,timeout=120)
 for folder in ('native/full','native/smoke'):
  run('reporters/report-native.py',folder)
 for folder in ('routes/full-shared','routes/full-unrelated','routes/smoke-shared'):
  run('reporters/report-routes.py',folder)
 run('reporters/report-cost.py','cost')
 for folder in ('native/full','native/smoke','routes/full-shared','routes/full-unrelated','routes/smoke-shared','cost'):
  for name in ('report.json','report.md'):
   require((root/folder/name).read_bytes()==(bundle/folder/name).read_bytes(),'report regeneration differs: '+folder+'/'+name)
 # Public harness smoke uses the collector's successful-process/cleanup contract;
 # its wrapper records exit status, rather than inventing a cleanup.json file.
 smoke=root/'qualification/public-fsync-smoke'
 require(json.loads((smoke/'completion.json').read_text())['exit_status']==0,'smoke process failed')
 sys.path.insert(0,str(root/'tests/benchmarks'))
 from comparison import expected_pairs,pair_digest,corpus_digest
 from compare_routes import foreign_nodes
 spec=json.loads((smoke/'spec.json').read_text());expected=expected_pairs(spec)
 records=[json.loads(s) for s in (smoke/'raw.jsonl').read_text().splitlines()]
 require(not any('failure' in r['kind'] for r in records),'smoke cleanup or control failed')
 settings=json.loads((smoke/'settings.json').read_text())
 require(settings['layout']=='byte_dispatch' and settings['ingest_durability']=='fsync','smoke configuration differs')
 require(len(settings['routes'])==6,'smoke route count differs')
 for route in settings['routes']:
  cohorts=[r for r in records if r['kind']=='cohort' and r['route']==route]
  require(len(cohorts)==2 and sum(c['warmup'] for c in cohorts)==1,'smoke cohort missing')
  for c in cohorts:
   require(len(c['results'])==1,'smoke result missing');r=c['results'][0]
   require('error' not in r,'smoke result failed')
   if route=='pg_backend':require(sum(n['Actual Rows']*n['Actual Loops'] for n in foreign_nodes(r['plan']['Plan']))==len(expected),'PG smoke rows differ')
   else:require(r['pairs']==len(expected) and r['pair_sha256']==pair_digest(expected),'smoke pair digest differs')
 settled=[r for r in records if r['kind']=='wire_settled']
 require(len(settled)==2 and all(r['corpus_sha256']==corpus_digest(spec) for r in settled),'smoke corpus differs')
print(f'PASS: {len(manifest)} payloads; six reports reproduced offline; six-route public smoke verified')
