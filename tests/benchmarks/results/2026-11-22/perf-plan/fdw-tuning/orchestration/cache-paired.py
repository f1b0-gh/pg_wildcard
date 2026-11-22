import hashlib,json,subprocess
from pathlib import Path
root=Path.cwd();base=root/'.local/perf/remaining-issues';out=base/'cache-paired';out.mkdir(exist_ok=False)
receipts=[]
for round_no in range(5):
    for variant in (('baseline','candidate') if round_no%2==0 else ('candidate','baseline')):
        binary=base/'binaries'/f'{variant}.so'
        temporary='/usr/lib/postgresql/18/lib/pg_wildcard-next.so'
        subprocess.run(['docker','cp',str(binary),'wildcard-review-pg18:'+temporary],check=True)
        subprocess.run(['docker','exec','wildcard-review-pg18','mv',temporary,'/usr/lib/postgresql/18/lib/pg_wildcard.so'],check=True)
        destination=out/f'{round_no}-{variant}'
        with (out/f'{round_no}-{variant}.log').open('w') as log:
            result=subprocess.run(['python3','tests/benchmarks/fdw_cache.py','--state',str(base/'antalya.private.json'),'--pg-container','wildcard-review-pg18','--output',str(destination),'--rounds','1','--modes','capacity','reuse','settings'],stdout=log,stderr=subprocess.STDOUT)
        receipt=dict(round=round_no,variant=variant,exit_status=result.returncode,sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
        receipts.append(receipt);(out/'receipts.json').write_text(json.dumps(receipts,indent=2)+'\n');print(json.dumps(receipt),flush=True)
        if result.returncode:raise SystemExit(result.returncode)
# Leave candidate installed for the insertion-TTL and terminal cohorts.
subprocess.run(['docker','cp',str(base/'binaries/candidate.so'),'wildcard-review-pg18:'+temporary],check=True)
subprocess.run(['docker','exec','wildcard-review-pg18','mv',temporary,'/usr/lib/postgresql/18/lib/pg_wildcard.so'],check=True)
with (out/'candidate-ttl.log').open('w') as log:
    result=subprocess.run(['python3','tests/benchmarks/fdw_cache.py','--state',str(base/'antalya.private.json'),'--pg-container','wildcard-review-pg18','--output',str(out/'candidate-ttl'),'--rounds','5','--modes','ttl'],stdout=log,stderr=subprocess.STDOUT)
raise SystemExit(result.returncode)
