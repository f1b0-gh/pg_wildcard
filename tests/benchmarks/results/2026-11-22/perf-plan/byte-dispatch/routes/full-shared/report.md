# Matched indexed route comparison

1,000,000 raw documents × 1,000 case-sensitive wildcard rules. All complete hit multisets match the independent Rust oracle. One source query for each native/direct/PG sweep; Elasticsearch returns the same pairs through 1,000 per-rule searches batched in msearch. Both engines have four CPUs and8GiB container limits; Elasticsearch has4GiB heap. No engine-efficiency inference is made from end-to-end client clocks.

| Route | Samples | Median ms [range] |
|---|---:|---:|
| native | 5 | 1982.730 [1854.154, 2155.172] |
| direct | 5 | 2657.454 [2496.907, 2807.427] |
| pg_backend | 5 | 2742.254 [2733.211, 2858.999] |
| pg_copy | 5 | 3119.934 [2929.887, 3196.824] |
| elastic_wildcard | 5 | 4171.747 [2890.426, 5030.294] |
| elastic_wildcard_4 | 5 | 1478.538 [1288.663, 2308.627] |

Direct uses published antalya-rs0.1.0; PostgreSQL backend execution and full binary COPY delivery remain separate. PG sweeps consume the generated exact pair query through the FDW, rather than adding local wc_* predicates. Native server durations and direct execution metrics are retained in report.json; first batch is not network TTFB and overlapping clocks must not be added/subtracted as isolated stages.

## Indexed ingestion through searchable visibility

**Ingestion comparability limit:** these runs used unequal acknowledgment durability: native insert/file-directory fsync disabled, Elasticsearch request-translog fsync enabled. The setup timings are default-setting observations, not a cross-engine ingestion performance ranking. Equal searchable visibility does not establish equal durability. A separate matched-fsync control is required. This does not change the settled read-query comparisons above.

Same raw documents,2,000-document HTTP batches,90% initial and10% incremental. Payload preparation is outside the upload clock and separately recorded. Elasticsearch includes a refresh; both clocks end after a count confirms searchable rows. These are endpoint ingestion costs, including protocol framing/response costs, not isolated indexing CPU. DDL and compaction are recorded separately. One build per engine per case, so no ingestion speedup significance claim.

| Engine | Phase | Documents | Upload+visibility seconds | Wire bytes |
|---|---|---:|---:|---:|
| native | initial | 900000 | 47.320 | 49250542 |
| native | incremental | 100000 | 5.199 | 5496850 |
| elastic | initial | 900000 | 80.359 | 73422782 |
| elastic | incremental | 100000 | 7.063 | 8195000 |

Settled native data-part bytes: 15,750,522; Elastic index-store bytes: 67,607,373. Both retain raw values and IDs; these are stored representations, not index-only or whole-server disk/RSS measurements.

One warmup per route, rotating measured order. Native/direct explicitly disable query result and condition caches; Elastic disables request/query caches. PG inherits the fixture cache policy: every recorded PG query has two condition-cache misses, with no hits recorded. The original raw metadata incorrectly said all query caches were disabled; it is preserved, and this correction plus per-route counters in report.json supersede that description. No cache-hit speedup is inferred. Host load, text-index/filesystem caches and runtime warming remain uncontrolled; observations and ranges are preserved. The1% smoke is correctness evidence, not a substitute for full-scale measurements.
