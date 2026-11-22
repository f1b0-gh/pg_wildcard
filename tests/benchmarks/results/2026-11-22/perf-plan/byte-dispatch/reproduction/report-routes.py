"""Offline verification/reporting of complete matched byte-index route cohorts."""
import hashlib,json,statistics,sys
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from comparison import corpus_digest,expected_pairs,pair_digest
from compare_routes import foreign_nodes
out=Path(sys.argv[1]);spec=json.loads((out/'spec.json').read_text());settings=json.loads((out/'settings.json').read_text())
assert json.loads((out/'cleanup.json').read_text())['failures']==[]
assert not (out/'pending.json').exists() and not (out/'failure.json').exists()
records=[json.loads(s) for s in (out/'raw.jsonl').read_text().splitlines()]
expected=expected_pairs(spec);digest=pair_digest(expected);summaries=[]
for route in settings['routes']:
 cohorts=[c for c in records if c['kind']=='cohort' and c['route']==route]
 assert len(cohorts)==settings['repeats']+1 and sum(c['warmup'] for c in cohorts)==1
 assert {c['repeat'] for c in cohorts if not c['warmup']}==set(range(settings['repeats']))
 results=[]
 for c in cohorts:
  assert not c['sampling_errors'] and len(c['results'])==1
  r=c['results'][0];assert 'error' not in r and r['request_ms']<60000
  if route=='pg_backend':assert sum(n['Actual Rows']*n['Actual Loops'] for n in foreign_nodes(r['plan']['Plan']))==len(expected)
  else:assert r['pairs']==len(expected) and r['pair_sha256']==digest
  if route.startswith('elastic_'):assert r['msearch_max_concurrent_searches']==settings['elastic_route_concurrency'][route]
  if not c['warmup']:results.append((c,r))
 times=[r['request_ms'] for c,r in results]
 server=[];terminal=[];decode=[];first=[]
 for c,r in results:
  profiles=r.get('profiles',[]) or c.get('profiles',[])
  if not route.startswith('elastic_'):
   assert len(profiles)==1
   server.append(sum(p['query_duration_ms'] for p in profiles))
  if route=='direct':
   assert len(r['executions'])==1
   e=r['executions'][0];terminal.append(e['terminal_ms'])
   if e['decode_ms'] is not None:decode.append(e['decode_ms'])
   if e['first_batch_ms'] is not None:first.append(e['first_batch_ms'])
 summaries.append(dict(route=route,samples=len(times),median_ms=statistics.median(times),min_ms=min(times),max_ms=max(times),native_server_ms=statistics.median(server) if server else None,pg_executor_ms=statistics.median([r['executor_ms'] for c,r in results]) if route=='pg_backend' else None,direct_terminal_ms=statistics.median(terminal) if terminal else None,direct_decode_ms=statistics.median(decode) if decode else None,direct_first_batch_ms=statistics.median(first) if first else None,pairs=len(expected)))
 profiles=[p for c in cohorts for r in c['results'] for p in (r.get('profiles',[]) or c.get('profiles',[]))]
 if route.startswith('pg_'):
  assert all(p['profile_events'].get('QueryConditionCacheMisses')==2 and not p['profile_events'].get('QueryConditionCacheHits',0) for p in profiles)
 def recorded_total(key):
  values=[p['profile_events'][key] for p in profiles if key in p['profile_events']]
  return sum(values) if values else None
 summaries[-1].update(condition_cache_explicitly_disabled=all(p['settings'].get('use_query_condition_cache')=='0' for p in profiles) if profiles else None,
                      condition_cache_misses_recorded=recorded_total('QueryConditionCacheMisses'),
                      condition_cache_hits_recorded=recorded_total('QueryConditionCacheHits'))
settled={r['engine']:r for r in records if r['kind']=='wire_settled'}
assert set(settled)=={'native','elastic'} and all(r['corpus_sha256']==corpus_digest(spec) for r in settled.values())
ingest=[r for r in records if r['kind']=='wire_ingest']
for engine in ('native','elastic'):
 selected=[r for r in ingest if r['engine']==engine]
 assert len(selected)==2 and sum(r['documents'] for r in selected)==spec['rows']
 assert {r['phase'] for r in selected}=={'initial','incremental'}
 assert next(r for r in selected if r['phase']=='incremental')['visible_rows']==spec['rows']
report=dict(rows=spec['rows'],indicators=spec['indicators'],corpus_sha256=corpus_digest(spec),pattern_sha256=spec['pattern_sha256'],routes=summaries,ingest=ingest,storage={'native_part_bytes':settled['native']['storage']['total_bytes'],'elastic_index_store_bytes':settled['elastic']['storage']['_all']['primaries']['store']['size_in_bytes']},raw_sha256=hashlib.sha256((out/'raw.jsonl').read_bytes()).hexdigest())
report['ingestion_comparable']=False
report['ingestion_comparability_reason']='Unequal default fsync acknowledgment durability; use a separately configured matched-fsync control.'
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
lines=['# Matched indexed route comparison','',f"{spec['rows']:,} raw documents × {spec['indicators']:,} case-sensitive wildcard rules. All complete hit multisets match the independent Rust oracle. One source query for each native/direct/PG sweep; Elasticsearch returns the same pairs through 1,000 per-rule searches batched in msearch. Both engines have four CPUs and8GiB container limits; Elasticsearch has4GiB heap. No engine-efficiency inference is made from end-to-end client clocks.",'','| Route | Samples | Median ms [range] |','|---|---:|---:|']
for r in summaries:lines.append(f"| {r['route']} | {r['samples']} | {r['median_ms']:.3f} [{r['min_ms']:.3f}, {r['max_ms']:.3f}] |")
lines+=['','Direct uses published antalya-rs0.1.0; PostgreSQL backend execution and full binary COPY delivery remain separate. PG sweeps consume the generated exact pair query through the FDW, rather than adding local wc_* predicates. Native server durations and direct execution metrics are retained in report.json; first batch is not network TTFB and overlapping clocks must not be added/subtracted as isolated stages.','', '## Indexed ingestion through searchable visibility','', 'Same raw documents,2,000-document HTTP batches,90% initial and10% incremental. Payload preparation is outside the upload clock and separately recorded. Elasticsearch includes a refresh; both clocks end after a count confirms searchable rows. These are endpoint ingestion costs, including protocol framing/response costs, not isolated indexing CPU. DDL and compaction are recorded separately. One build per engine per case, so no ingestion speedup significance claim.','','| Engine | Phase | Documents | Upload+visibility seconds | Wire bytes |','|---|---|---:|---:|---:|']
for r in ingest:lines.append(f"| {r['engine']} | {r['phase']} | {r['documents']} | {r['searchable_seconds']:.3f} | {r['wire_bytes']} |")
lines+=['',f"Settled native data-part bytes: {report['storage']['native_part_bytes']:,}; Elastic index-store bytes: {report['storage']['elastic_index_store_bytes']:,}. Both retain raw values and IDs; these are stored representations, not index-only or whole-server disk/RSS measurements.",'', 'One warmup per route, rotating measured order. Native/direct explicitly disable query result and condition caches; Elastic disables request/query caches. PG inherits the fixture cache policy: every recorded PG query has two condition-cache misses, with no hits recorded. The original raw metadata incorrectly said all query caches were disabled; it is preserved, and this correction plus per-route counters in report.json supersede that description. No cache-hit speedup is inferred. Host load, text-index/filesystem caches and runtime warming remain uncontrolled; observations and ranges are preserved. The1% smoke is correctness evidence, not a substitute for full-scale measurements.']
lines.insert(lines.index('## Indexed ingestion through searchable visibility')+1, '\n'+'**Ingestion comparability limit:** these runs used unequal acknowledgment durability: native insert/file-directory fsync disabled, Elasticsearch request-translog fsync enabled. The setup timings are default-setting observations, not a cross-engine ingestion performance ranking. Equal searchable visibility does not establish equal durability. A separate matched-fsync control is required. This does not change the settled read-query comparisons above.')
(out/'report.md').write_text('\n'.join(lines)+'\n')
print('Verified',len(summaries),'complete route cohorts, identical source digests and wire-ingest records')
