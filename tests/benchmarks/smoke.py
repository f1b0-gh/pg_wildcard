#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""One-percent semantic smoke. Timings are artifacts, never performance gates."""
import argparse, json, subprocess, sys
from pathlib import Path

p=argparse.ArgumentParser(description=__doc__);p.add_argument('spec',type=Path);p.add_argument('--out',type=Path,required=True);a=p.parse_args()
a.out.mkdir(parents=True,exist_ok=True)
root=Path(__file__).resolve().parents[2]
runner=root/'libwildcard/tests/benchmarks/sweeps.py'
spec=json.loads(a.spec.read_text())
for mode, strategies in [('sweeps',spec['queries']),('ingest',spec['watchlist']['mvs']),('candidates',['all'])]:
    for strategy in strategies:
        dest=a.out/f'{mode}-{strategy}.jsonl'
        with a.spec.open() as source,dest.open('w') as target:
            result=subprocess.run([sys.executable,str(runner),'--mode',mode,'--strategy',strategy,'--rows','10000','--repeats','1']+(['--all-indicator-canary'] if mode=='sweeps' else []),stdin=source,stdout=target,stderr=subprocess.STDOUT)
        records=[]
        for line in dest.read_text().splitlines():
            try:records.append(json.loads(line))
            except ValueError:pass
        if result.returncode or any(r.get('kind')=='failure' for r in records):
            print(dest.read_text());raise SystemExit(f'{mode}/{strategy} failed correctness/operation')
        summaries=[r for r in records if r.get('kind') in ['summary','candidate_pairs']]
        if not summaries:raise SystemExit('missing measurement output')
        for summary in summaries:print(json.dumps(summary),flush=True)
print('Benchmark smoke: PASS (no timing thresholds)',flush=True)
