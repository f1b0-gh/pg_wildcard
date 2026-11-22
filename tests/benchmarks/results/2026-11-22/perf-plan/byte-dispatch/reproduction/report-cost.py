# SPDX-License-Identifier: Apache-2.0
"""Validate and report the two completed fsync ingestion controls offline."""
import hashlib,json,sys
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from comparison import corpus_digest
root=Path(sys.argv[1]);rows=[];sources={}
for family in ('shared','unrelated'):
 base=root/('fsync-full-'+family)
 assert json.loads((base/'cleanup.json').read_text())['failures']==[]
 assert not (base/'failure.json').exists()
 spec=json.loads((base/'spec.json').read_text());settings=json.loads((base/'settings.json').read_text())
 assert spec['rows']==1_000_000 and spec['indicators']==1000
 assert settings['corpus_sha256']==corpus_digest(spec)
 assert hashlib.sha256((base/'indexed_ingest.py').read_bytes()).hexdigest()==settings['source_sha256']
 ddl=(base/'native-confirmed-ddl.sql').read_text()
 assert 'fsync_after_insert = 1' in ddl and 'fsync_part_directory = 1' in ddl
 elastic=json.loads((base/'elastic-confirmed-settings.json').read_text())
 assert elastic['settings']['index.translog.durability'].lower()=='request'
 resources=settings['resources'];assert {r['engine'] for r in resources}=={'antalya','elasticsearch'}
 assert all(r['cpus']==4 and r['memory_bytes']==8<<30 for r in resources)
 assert next(r['heap_bytes'] for r in resources if r['engine']=='elasticsearch')==4<<30
 raw=(base/'raw.jsonl').read_bytes();records=[json.loads(line) for line in raw.splitlines()]
 sources[family]=hashlib.sha256(raw).hexdigest()
 settled={r['engine']:r for r in records if r['kind']=='wire_settled'}
 assert set(settled)=={'native','elastic'}
 assert all(r['corpus_sha256']==corpus_digest(spec) for r in settled.values())
 for engine in ('native','elastic'):
  ingest=[r for r in records if r['kind']=='wire_ingest' and r['engine']==engine]
  assert len(ingest)==2
  by_phase={r['phase']:r for r in ingest}
  for phase,documents,visible,requests in [('initial',900000,900000,450),('incremental',100000,1000000,50)]:
   r=by_phase[phase]
   assert (r['documents'],r['visible_rows'],r['requests'])==(documents,visible,requests)
   assert r['searchable_seconds']>=r['upload_ack_seconds']>=0
   rows.append(dict(family=family,engine=engine,**{k:r[k] for k in ('phase','documents','requests','wire_bytes','client_prepare_seconds','upload_ack_seconds','searchable_seconds')}))
report={'observations':rows,'raw_sha256':sources,'repetitions_per_case':1,'contract':'configured fsync acknowledgment and count-confirmed visibility; no hardware/crash test'}
(root/'report.json').write_text(json.dumps(report,indent=2)+'\n')
lines=['# Indexed ingestion with configured fsync','', 'Same raw documents, 2,000-document HTTP batches, index maintained on insert, four CPU / eight GiB data-engine containers. Native enables inserted-part and part-directory fsync; Elastic uses request-translog durability and a four GiB heap. Both acknowledge configured fsync before the upload returns. Count-confirmed visibility includes Elastic refresh. This is not a hardware crash test.','', '| Fixture | Engine | Initial 900k seconds | Incremental 100k seconds |','|---|---|---:|---:|']
for family in ('shared','unrelated'):
 for engine in ('native','elastic'):
  observations={r['phase']:r for r in rows if r['family']==family and r['engine']==engine}
  lines.append(f"| {family} | {engine} | {observations['initial']['searchable_seconds']:.3f} | {observations['incremental']['searchable_seconds']:.3f} |")
lines+=['','One observation per engine/fixture, always native then Elastic. These are not medians, confidence intervals or maximum ingestion throughput. Serialization, compaction, source verification and stored representations are separate in raw.jsonl. Both final corpus digests match the independent generator. Machine/settings, exact DDL, effective durability and cleanup records are retained.','', 'The earlier default-setting route setup costs have different acknowledgment contracts and remain unsuitable for a cross-engine ingestion ranking. In particular, the much longer earlier unrelated Elastic load was not reproduced in this control; no specific cause has been isolated. Host/cache/load/order variation prevents attributing that difference solely to the fsync setting. These single controls do not change the separately measured query latency cohorts.']
(root/'report.md').write_text('\n'.join(lines)+'\n')
print('Verified two fsync controls, eight phase observations, equal corpus digests and resource settings')
