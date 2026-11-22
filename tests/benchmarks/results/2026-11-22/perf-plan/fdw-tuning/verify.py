# SPDX-License-Identifier: Apache-2.0
"""Check exact membership/digests, observations and regenerated tables offline."""
import gzip,hashlib,json,subprocess,sys,tempfile
from pathlib import Path
bundle=Path(__file__).resolve().parent

def require(ok,message):
    if not ok:raise ValueError(message)
manifest=json.loads((bundle/'sha256.json').read_text())
require({str(p.relative_to(bundle)) for p in bundle.rglob('*') if p.is_file()}==set(manifest)|{'sha256.json'},'manifest membership mismatch')
for name,sha in manifest.items():
    path=Path(name);require(not path.is_absolute() and '..' not in path.parts,'unsafe path')
    require(hashlib.sha256((bundle/path).read_bytes()).hexdigest()==sha,'hash mismatch: '+name)
for name,p in json.loads((bundle/'provenance.json').read_text()).items():
    raw=(bundle/name).read_bytes();raw=gzip.decompress(raw) if p['gzip'] else raw
    require(hashlib.sha256(raw).hexdigest()==p['source_sha256'],'source digest mismatch')
for path in bundle.glob('*/cleanup.json'):
    state=json.loads(path.read_text())
    if path.parent.name=='streaming-full':
        require(json.loads((path.parent/'cleanup-recovery.json').read_text())['status']=='complete','original cleanup not recovered')
        require(json.loads((path.parent/'correlation-recovery.json').read_text())['replayed_measurements'] is False,'timing replayed')
    else:require(not state['failures'],'cleanup failed')
for path in (bundle/'cache-paired').glob('*/cleanup.json'):require(not json.loads(path.read_text())['failures'],'cache cleanup failed')
require(json.loads((bundle/'terminal-100/cadence-receipt.json').read_text())['restored'] is True,'cadence not restored')
require(json.loads((bundle/'streaming-wide/resource-refusal.json').read_text())['complete_performance_cohort'] is False,'wide refusal promoted to result')
with tempfile.TemporaryDirectory(prefix='fdw-tuning-evidence-') as directory:
    destination=Path(directory)
    # Work from disposable copies: reporting never modifies the source evidence.
    import shutil
    shutil.copytree(bundle,destination/'evidence');copy=destination/'evidence'
    wrapper="import runpy,sys\ndef audit(event,args):\n if event in ('socket.connect','subprocess.Popen','os.system','os.posix_spawn','os.posix_spawnp'):raise RuntimeError('report attempted external action')\nsys.addaudithook(audit)\nsys.argv=sys.argv[1:]\nrunpy.run_path(sys.argv[0],run_name='__main__')\n"
    subprocess.run([sys.executable,'-B','-c',wrapper,str(copy/'report.py'),str(copy)],check=True,timeout=60)
    for name in ('report.json','tables.md'):require((copy/name).read_bytes()==(bundle/name).read_bytes(),'regeneration differs: '+name)
print('PASS: payload hashes, exact cohorts, value/completion controls, pruning, TTL and regenerated tables')
