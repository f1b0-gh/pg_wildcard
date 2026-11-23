# SPDX-License-Identifier: Apache-2.0
"""Run owned baseline/candidate profiles sequentially after latency measurement."""
import subprocess
from pathlib import Path
root=Path('.local/followups-20261123')
def install(name):
    temporary='/usr/lib/postgresql/18/lib/pg_wildcard.so.profile'
    subprocess.run(['docker','exec','wildcard-review-pg18','cp','/workspace/'+str(root/name),temporary],check=True)
    subprocess.run(['docker','exec','wildcard-review-pg18','mv',temporary,'/usr/lib/postgresql/18/lib/pg_wildcard.so'],check=True)
try:
    for label,name in [('baseline','baseline.so'),('optimized','candidate.so')]:
        install(name)
        print('START',label,flush=True)
        subprocess.run(['python3',str(root/'fdw-flames-one.py'),'--label',label],check=True)
        print('END',label,flush=True)
finally:
    install('candidate.so')
