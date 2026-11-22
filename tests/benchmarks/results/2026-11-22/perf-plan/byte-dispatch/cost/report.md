# Indexed ingestion with configured fsync

Same raw documents, 2,000-document HTTP batches, index maintained on insert, four CPU / eight GiB data-engine containers. Native enables inserted-part and part-directory fsync; Elastic uses request-translog durability and a four GiB heap. Both acknowledge configured fsync before the upload returns. Count-confirmed visibility includes Elastic refresh. This is not a hardware crash test.

| Fixture | Engine | Initial 900k seconds | Incremental 100k seconds |
|---|---|---:|---:|
| shared | native | 64.147 | 7.699 |
| shared | elastic | 76.990 | 8.320 |
| unrelated | native | 85.060 | 7.280 |
| unrelated | elastic | 59.064 | 5.911 |

One observation per engine/fixture, always native then Elastic. These are not medians, confidence intervals or maximum ingestion throughput. Serialization, compaction, source verification and stored representations are separate in raw.jsonl. Both final corpus digests match the independent generator. Machine/settings, exact DDL, effective durability and cleanup records are retained.

The earlier default-setting route setup costs have different acknowledgment contracts and remain unsuitable for a cross-engine ingestion ranking. In particular, the much longer earlier unrelated Elastic load was not reproduced in this control; no specific cause has been isolated. Host/cache/load/order variation prevents attributing that difference solely to the fsync setting. These single controls do not change the separately measured query latency cohorts.
