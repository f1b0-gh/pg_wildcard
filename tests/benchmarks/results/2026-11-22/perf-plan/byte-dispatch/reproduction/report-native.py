"""Regenerate exact-pair indexed sweep reports from completed raw measurements."""
import gzip,hashlib,json,statistics,sys
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from comparison import expected_pairs,pair_digest
root=Path(sys.argv[1]);output=[]
for family in ('shared','unrelated'):
 base=root/family
 failure=json.loads((base/'failure.json').read_text()) if (base/'failure.json').exists() else None
 assert failure is not None or not (base/'pending.json').exists()
 assert json.loads((base/'cleanup.json').read_text())['failures']==[]
 spec=json.loads(gzip.decompress((base/'spec.json.gz').read_bytes()));expected=expected_pairs(spec)
 settings=json.loads((base/'settings.json').read_text());repeats=settings['repeats']
 records=[json.loads(line) for line in (base/'raw.jsonl').read_text().splitlines()]
 records=[r for r in records if 'strategy' in r]
 summaries=json.loads((base/'summary.json').read_text()) if (base/'summary.json').exists() else None
 for label in json.loads(gzip.decompress((base/'variants.json.gz').read_bytes())):
  selected=[r for r in records if r['strategy']==label and not r['warmup']]
  assert len(selected)<=repeats and len({r['repeat'] for r in selected})==len(selected)
  if failure is None:assert len(selected)==repeats and {r['repeat'] for r in selected}==set(range(repeats))
  assert sum(r['warmup'] for r in records if r['strategy']==label) <= 1
  if failure is None: assert sum(r['warmup'] for r in records if r['strategy']==label)==1
  assert all(r['pairs']==len(expected) and r['pair_sha256']==pair_digest(expected) for r in records if r['strategy']==label)
  assert all(len(r['profiles'])==len(r['query_ids'])==r['source_queries'] for r in selected)
  times=[r['request_ms'] for r in selected];reads=[sum(p['read_rows'] for p in r['profiles']) for r in selected];sizes=[sum(p['read_bytes'] for p in r['profiles']) for r in selected]
  if summaries is not None:assert summaries[label]==dict(samples=repeats,median_ms=statistics.median(times),min_ms=min(times),max_ms=max(times),read_rows=reads,read_bytes=sizes)
  events={}
  for r in selected:
   for p in r['profiles']:
    for key,value in p['profile_events'].items():
     if 'TextIndex' in key:events[key]=events.get(key,0)+value
  if not selected:
   output.append(dict(family=family,strategy=label,samples=0,status='censored',failure=failure))
   continue
  output.append(dict(family=family,strategy=label,rows=spec['rows'],indicators=spec['indicators'],samples=len(selected),status='complete' if len(selected)==repeats else 'partial',failure=failure,pairs=len(expected),
   median_ms=statistics.median(times),min_ms=min(times),max_ms=max(times),request_count=selected[0]['source_queries'],source_references=selected[0]['source_references'],
   median_read_rows=statistics.median(reads),median_read_bytes=statistics.median(sizes),max_query_memory_bytes=max(p['memory_usage'] for r in selected for p in r['profiles']),text_index_events_sum=events,
   raw_sha256=hashlib.sha256((base/'raw.jsonl').read_bytes()).hexdigest()))
(root/'report.json').write_text(json.dumps(output,indent=2)+'\n')
lines=['# Required-fragment byte-index dispatch experiment','',
 'Native HTTP complete-delivery times on identical corpora in separate indexed tables (existing n-gram layout versus bounded hex-fragment layout), with complete row/indicator/view checks outside the timer. Each strategy has one warmup followed by the stated repetitions in rotating order. No Elasticsearch, direct antalya-rs or PostgreSQL timing is inferred from this experiment.','',
 '| Fixture | Strategy | Samples | Median ms [range] | Requests | Source references | Median logical rows read | Median read bytes |',
 '|---|---|---:|---:|---:|---:|---:|---:|']
for r in output:
 if not r['samples']:
  lines.append(f"| {r['family']} | {r['strategy']} | 0 (censored) | unavailable | — | — | — | — |")
  continue
 lines.append(f"| {r['family']} | {r['strategy']} | {r['samples']} ({r['status']}) | {r['median_ms']:.3f} [{r['min_ms']:.3f}, {r['max_ms']:.3f}] | {r['request_count']} | {r['source_references']} | {r['median_read_rows']:.0f} | {r['median_read_bytes']:.0f} |")
lines+=['','Every successful sample is checked against the complete independent Rust pair oracle. Partial cohorts and deadlines are not speedup measurements. All strategies retain exact regex checks. The dispatch layout uses bounded hex-encoded byte fragments and an overflow token; windows crossing half-byte boundaries may add candidates. Index activity is preserved in raw profiles and report.json. Logical reads are not physical disk I/O, and per-query memory is not whole-server RSS. Host load and filesystem caches are uncontrolled.','',
 'byte-index-build.json records native index build, incremental server-to-server insertion and storage. These are not end-to-end Elastic ingestion measurements. Comparing index costs fairly still requires matched indexed ingestion/searchable visibility and resource settings. No Elasticsearch, direct published antalya-rs or PostgreSQL timing is inferred here.']
lines += ['', 'Source-gate diagnostics (forced text index, outside timed requests):', '',
 '| Fixture | One-fragment candidates | Required-fragment candidates | Source rows |',
 '|---|---:|---:|---:|']
for family in ('shared','unrelated'):
 gates=json.loads((root/family/'source-gates.json').read_text())
 assert all(g['forced_index'] for g in gates.values())
 lines.append(f"| {family} | {gates['byte_dispatch_coarse']['candidate_rows']} | {gates['byte_dispatch']['candidate_rows']} | {gates['byte_dispatch']['source_rows']} |")
lines += ['', 'Both byte-dispatch variants use the same physical byte-fragment index. Their cost difference is query-side. The refined query requires up to three proven fragments per rule and retains overflow plus exact regex checks. Logical read_bytes includes index-derived virtual boolean columns; it is not a measure of physical storage traffic. read_rows alone does not establish whether a text index was used: the saved EXPLAIN actions and profiles supply that evidence. The 10k-row smoke shows extra query overhead for the refined gate, so these results do not support a universal speedup.']
(root/'report.md').write_text('\n'.join(lines)+'\n')
print('Verified',len(output),'strategy outcomes including explicit partial/censored results')
