"""Interleave release binaries; never replace a mapped library inode in place."""
import hashlib,json,subprocess,time
from pathlib import Path
root=Path('.local/perf/fdw');root.mkdir(exist_ok=True)
binaries={'baseline':Path('.local/perf-review/fdw/pg_wildcard-measured-release.so'),'prepared':Path('.local/perf/execution/pg-prepared.so')}
status=[]
def install(label):
 source='/workspace/'+str(binaries[label]);temporary='/usr/lib/postgresql/18/lib/pg_wildcard.so.wc-perf'
 subprocess.run(['docker','exec','wildcard-review-pg18','cp',source,temporary],check=True)
 subprocess.run(['docker','exec','wildcard-review-pg18','mv',temporary,'/usr/lib/postgresql/18/lib/pg_wildcard.so'],check=True)
try:
 for repeat in range(5):
  labels=['baseline','prepared'] if repeat%2==0 else ['prepared','baseline']
  for label in labels:
   if Path('.local/perf/execution/timing-active').exists():raise RuntimeError('another timing window active')
   install(label);name=f'{label}-{repeat}';out=root/name
   print('START',name,flush=True);started=time.monotonic()
   with (root/(name+'.log')).open('w') as log:
    r=subprocess.run(['python3','.local/perf/execution/fdw-benchmark.py','--label',label,'--out',str(out),'--repeats','1'],stdout=log,stderr=subprocess.STDOUT,timeout=900)
   status.append(dict(name=name,exit_status=r.returncode,seconds=time.monotonic()-started,binary_sha256=hashlib.sha256(binaries[label].read_bytes()).hexdigest()))
   (root/'comparison-status.json').write_text(json.dumps(status,indent=2)+'\n');print('END',name,r.returncode,flush=True)
   if r.returncode:raise SystemExit('stop to inspect FDW run failure')
finally:
 install('baseline')
