"""Interleaved exact release binaries; all builds must finish before this run."""
import hashlib,json,subprocess,time
from pathlib import Path
root=Path('.local/followups-20261123')
binaries={'baseline':root/'baseline.so','optimized':root/'candidate.so'}
status=[]
def install(label):
 source='/workspace/'+str(binaries[label]); temporary='/usr/lib/postgresql/18/lib/pg_wildcard.so.overhead'
 subprocess.run(['docker','exec','wildcard-review-pg18','cp',source,temporary],check=True)
 subprocess.run(['docker','exec','wildcard-review-pg18','mv',temporary,'/usr/lib/postgresql/18/lib/pg_wildcard.so'],check=True)
try:
 for repeat in range(5):
  labels=['baseline','optimized'] if repeat%2==0 else ['optimized','baseline']
  for label in labels:
   if Path('.local/perf/execution/timing-active').exists():raise RuntimeError('another timing window active')
   install(label);name=f'{label}-{repeat}';out=root/'cohorts'/name
   out.parent.mkdir(exist_ok=True)
   print('START',name,flush=True);started=time.monotonic()
   with (out.parent/(name+'.log')).open('w') as log:
    result=subprocess.run(['python3',str(root/'fdw-benchmark.py'),'--label',label,'--out',str(out),'--repeats','1','--policy','in_band'],stdout=log,stderr=subprocess.STDOUT,timeout=900)
   status.append(dict(name=name,exit_status=result.returncode,seconds=time.monotonic()-started,binary_sha256=hashlib.sha256(binaries[label].read_bytes()).hexdigest()))
   (root/'comparison-status.json').write_text(json.dumps(status,indent=2)+'\n');print('END',name,result.returncode,flush=True)
   if result.returncode:raise SystemExit('measurement failed; inspect cohort log')
finally:
 install('optimized')
