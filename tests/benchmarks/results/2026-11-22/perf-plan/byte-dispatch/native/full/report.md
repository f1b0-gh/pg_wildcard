# Required-fragment byte-index dispatch experiment

Native HTTP complete-delivery times on identical corpora in separate indexed tables (existing n-gram layout versus bounded hex-fragment layout), with complete row/indicator/view checks outside the timer. Each strategy has one warmup followed by the stated repetitions in rotating order. No Elasticsearch, direct antalya-rs or PostgreSQL timing is inferred from this experiment.

| Fixture | Strategy | Samples | Median ms [range] | Requests | Source references | Median logical rows read | Median read bytes |
|---|---|---:|---:|---:|---:|---:|---:|
| shared | constant_candidates | 5 (complete) | 3535.383 [3305.452, 4203.954] | 2 | 2 | 2000000 | 95680004 |
| shared | byte_dispatch | 5 (complete) | 1629.951 [1306.593, 1766.477] | 1 | 1 | 1000000 | 1045840002 |
| shared | byte_dispatch_coarse | 5 (complete) | 1820.368 [1696.364, 1910.582] | 1 | 1 | 1000000 | 45840002 |
| unrelated | constant_candidates | 5 (complete) | 26188.486 [23952.073, 28429.429] | 2 | 2 | 2000000 | 89780004 |
| unrelated | byte_dispatch | 5 (complete) | 2072.031 [1597.941, 2344.040] | 1 | 1 | 1000000 | 1045890002 |
| unrelated | byte_dispatch_coarse | 5 (complete) | 9848.770 [8508.563, 10216.236] | 1 | 1 | 1000000 | 45890002 |

Every successful sample is checked against the complete independent Rust pair oracle. Partial cohorts and deadlines are not speedup measurements. All strategies retain exact regex checks. The dispatch layout uses bounded hex-encoded byte fragments and an overflow token; windows crossing half-byte boundaries may add candidates. Index activity is preserved in raw profiles and report.json. Logical reads are not physical disk I/O, and per-query memory is not whole-server RSS. Host load and filesystem caches are uncontrolled.

byte-index-build.json records native index build, incremental server-to-server insertion and storage. These are not end-to-end Elastic ingestion measurements. Comparing index costs fairly still requires matched indexed ingestion/searchable visibility and resource settings. No Elasticsearch, direct published antalya-rs or PostgreSQL timing is inferred here.

Source-gate diagnostics (forced text index, outside timed requests):

| Fixture | One-fragment candidates | Required-fragment candidates | Source rows |
|---|---:|---:|---:|
| shared | 269281 | 10000 | 1000000 |
| unrelated | 921471 | 13407 | 1000000 |

Both byte-dispatch variants use the same physical byte-fragment index. Their cost difference is query-side. The refined query requires up to three proven fragments per rule and retains overflow plus exact regex checks. Logical read_bytes includes index-derived virtual boolean columns; it is not a measure of physical storage traffic. read_rows alone does not establish whether a text index was used: the saved EXPLAIN actions and profiles supply that evidence. The 10k-row smoke shows extra query overhead for the refined gate, so these results do not support a universal speedup.
