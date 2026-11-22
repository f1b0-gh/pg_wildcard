<!-- SPDX-License-Identifier: Apache-2.0 -->
# Measured search and FDW routes — 2026-11-22

The results distinguish native engine SQL, direct published `antalya-rs`,
PostgreSQL backend execution, PostgreSQL protocol delivery, and Elasticsearch.
All are synthetic measurements on a shared development host. Raw records,
settings, exact-result checks, sample counts and limitations accompany each table.

## One million URLs, one thousand literal indicators

All completed sweeps produced the same 10,000 row/indicator/view pairs.

| Route / strategy | Median seconds | Samples |
|---|---:|---:|
| Native HTTP SQL, previous fixed-64 sweeps | 13.958 | 3 |
| Native HTTP SQL, final shared-gram / AST-budgeted sweeps | 4.043 | 3 |
| Elasticsearch `wildcard` field, 1,000 wildcard queries | 1.575 | 3 |
| Elasticsearch `keyword` field, 1,000 wildcard queries | 7.298 | 3 |

The native source is an unindexed String column in an id-ordered MergeTree,
about 9.3 MB on disk. Elasticsearch uses separate searchable indexes: 67.6 MB
for `wildcard`, 25.4 MB for `keyword`, one shard and no replicas. Its sweeps use
ten `_msearch` requests with up to four concurrent searches and include result
validation in their wall clock; native validation is untimed. These are useful
workload comparisons, not equal index layouts or identical timer boundaries.
The corpus's shared rare grams favor the native prefilter; general RE2/Unicode
and invalid-UTF-8 equivalence with Elasticsearch is not claimed.

Native scans fell from sixteen to two. The final ingest-time watchlist took
106.9 seconds with heuristic ranking and 83.0 seconds with file-frequency
ranking, each one completed sample; the old lexical and exhaustive variants
exceeded 300 seconds. Elasticsearch indexing and boolean-union timings are
separate operations, not equivalents of this insert-triggered watchlist.

See [native sweep/ingest evidence](sweeps/README.md) and
[Elasticsearch evidence and semantic probes](elastic/README.md).

## Direct Antalya versus PostgreSQL

This is a separate paired FDW fixture, not the thousand-indicator sweep above.
The direct worker uses the published `antalya-rs =0.1.0` client and consumes
Arrow batches. PostgreSQL uses that same client, converts values, and retains
local exact conditions. Protocol delivery additionally returns binary COPY data
to a container-local psql client over TCP loopback.

| One-million-row workload | Direct client process ms | PG backend `TIMING OFF` ms | Full psql process / delivery ms |
|---|---:|---:|---:|
| One integer column | 231.5 | 381.2 | 693.3 |
| Three columns with broad wildcard local rechecks | 208.6 | 916.4 | 1813.0 |

Each entry is a three-sample median. Process initialization, formats and timer
boundaries differ; these columns must not be subtracted into exclusive stage
costs. The report also preserves fetch-only clocks, ranges, Docker-forwarded
controls, real 1 KiB rows, lake pruning, all terminal policies, requested 1/8/32
client concurrency, and adaptive cold/warm preparation.

Per-row EXPLAIN timing substantially distorted throughput on this host, so
TIMING ON measurements serve as diagnostics. A real CPU flame graph used
TIMING OFF. Individual batch charges respected their limits, but backend RSS
is not bounded by a batch limit; true HTTP TTFB and aggregate retained-byte
peaks were unavailable through the published API. Lake pruning skipped seven
of eight files, while confirmation latency sometimes masked that saving.

See the [full FDW report, stage tables and flame graph](fdw/README.md).
Follow-up issues cover [conversion](https://github.com/f1b0-gh/pg_wildcard/issues/17),
[streaming attribution](https://github.com/f1b0-gh/pg_wildcard/issues/18),
[terminal confirmation](https://github.com/f1b0-gh/pg_wildcard/issues/19), and
[cold adaptive preparation](https://github.com/f1b0-gh/pg_wildcard/issues/20).
