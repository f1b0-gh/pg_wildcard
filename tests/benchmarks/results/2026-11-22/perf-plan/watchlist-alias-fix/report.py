"""Validate the helper-projection performance transfer without engine access."""
import gzip,hashlib,json,statistics,sys
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from comparison import expected_pairs,pair_digest
root=Path(sys.argv[1]);rows=[]
for family in ('shared','unrelated'):
 d=root/'measurements'/family
 assert not (d/'pending.json').exists()
 assert json.loads((d/'cleanup.json').read_text())['failures']==[]
 spec=json.loads(gzip.decompress((d/'spec.json.gz').read_bytes()).decode());expected=expected_pairs(spec)
 records=[json.loads(line) for line in (d/'raw.jsonl').read_text().splitlines()]
 records=[r for r in records if 'strategy' in r]
 groups={label:[r for r in records if r['strategy']==label and not r['warmup']] for label in ('before_alias_fix','isolated_projection')}
 summary=json.loads((d/'summary.json').read_text())
 for label,selected in groups.items():
  assert len(selected)==5 and {r['repeat'] for r in selected}==set(range(5))
  assert sum(r['warmup'] for r in records if r['strategy']==label)==1
  assert all(r['pairs']==len(expected) and r['pair_sha256']==pair_digest(expected) for r in selected)
  values=[r['request_ms'] for r in selected]
  assert summary[label]==dict(samples=5,median_ms=statistics.median(values),min_ms=min(values),max_ms=max(values))
 a=groups['before_alias_fix'];b={r['repeat']:r for r in groups['isolated_projection']}
 paired=[100*(b[r['repeat']]['request_ms']/r['request_ms']-1) for r in a]
 change=100*(summary['isolated_projection']['median_ms']/summary['before_alias_fix']['median_ms']-1)
 rows.append(dict(family=family,rows=spec['rows'],indicators=spec['indicators'],pairs=len(expected),summary=summary,
  median_ratio_change_percent=change,median_paired_change_percent=statistics.median(paired),
  investigation_required=change>10,
  raw_sha256=hashlib.sha256((d/'raw.jsonl').read_bytes()).hexdigest()))
(root/'report.json').write_text(json.dumps(rows,indent=2)+'\n')
lines=['# Watchlist helper-projection transfer check','',
 'Five interleaved pairs after warmup, one million source rows and 1,000 rules per fixture. '
 'Both variants use the same previous production rule join; only the source helper projection changes. '
 'The complete native INSERT SELECT persists all hit pairs through the actual MV. '
 'Exact validation is outside the timer. This is not a new regex or Elastic comparison.','',
 '| Fixture | Before fix ms median [range] | Isolated projection ms median [range] | Median ratio change | Median paired change |',
 '|---|---:|---:|---:|---:|']
for row in rows:
 def cell(label):
  r=row['summary'][label];return f"{r['median_ms']:.3f} [{r['min_ms']:.3f}, {r['max_ms']:.3f}]"
 lines.append(f"| {row['family']} | {cell('before_alias_fix')} | {cell('isolated_projection')} | {row['median_ratio_change_percent']:+.2f}% | {row['median_paired_change_percent']:+.2f}% |")
lines+=['','The source/rules are identical to the earlier production-transfer fixtures. '
 'Each measured request returns the exact 10,000-pair multiset and has an original native profile. '
 'Host load and filesystem caches remain uncontrolled; this small transfer check does not '
 'establish a general speedup from the correctness fix. A median regression above 10% '
 'requires investigation. Historical timings and failed controls remain unchanged.']
(root/'report.md').write_text('\n'.join(lines)+'\n')
print(json.dumps([dict(family=r['family'],change_percent=r['median_ratio_change_percent'],investigation_required=r['investigation_required']) for r in rows]))
