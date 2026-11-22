<!-- SPDX-License-Identifier: Apache-2.0 -->
# Native sweeps and ingest: 1M rows × 1,000 indicators

The final sweep uses two source queries and a shared prefilter proved necessary
for every indicator. It took **4.043 seconds**, versus **13.958 seconds** for the
previous fixed-64 batching, with the same 10,000 exact row/indicator/view pairs.
Native `system.query_log` recorded 2,000,000 versus 16,000,000 rows read. This
meets the scan-count and latency targets for this fixture.

These measurements use native engine SQL over HTTP, **not `antalya-rs`**.
The server is Antalya 26.8.34.20001, with a four-CPU quota and 8 GiB container
limit; queries use four threads, 8,192-row blocks and a 4,000,000,000-byte memory
limit. The host is an AMD Ryzen 5 PRO 4650U with 12 visible logical CPUs and
about 15 GiB RAM. Settings and machine details are retained in every JSONL log.

## Batch sweep results

Elapsed time includes executing every query and receiving all matching pairs.
Setup and oracle checks are outside the timer. Rates count the million input
rows once, irrespective of repeated source scans. Peaks are the query-log
memory measurement for one query, not whole-server RSS.

| Strategy | Samples | Median seconds | Input rows/s | Source queries | Rows read | Peak query MiB |
|---|---:|---:|---:|---:|---:|---:|
| Fixed 64 indicators, previous implementation | 3 | 13.958 | 71,643 | 16 | 16,000,000 | 121.0 |
| **Shared required grams + byte/AST budgets, final** | **3** | **4.043** | **247,341** | **2** | **2,000,000** | **89.8** |
| Exhaustive exact matching | 1 | 142.962 | 6,995 | 1 | 1,000,000 | 2954.2 |
| Historical Hyperscan precheck, ASCII baseline | 3 | 1.225 | 816,422 | 1 | 1,000,000 | 12.9 |
| Budgeted dense tuples without shared gate, rejected | 3 | 16.651 | 60,058 | 2 | 2,000,000 | 1078.7 |
| Sparse conditional arrays, rejected | 3 | 36.382 | 27,486 | 2 | 2,000,000 | 445.0 |
| Filtered 64-item groups in larger queries, rejected | 1 | 19.267 | 51,903 | 2 | 2,000,000 | 425.2 |

The final filter uses the data-derived shared grams `il0`, `evi`, and `vil`.
Every indicator in this synthetic corpus requires them. Native EXPLAIN confirms
that the ordinary WHERE condition becomes a PREWHERE filter before ARRAY JOIN.
The exact per-indicator predicates remain. Mixed views, unsupported plans,
ambiguous aliases, custom expressions or insufficient remaining query budget
omit this optional gate. File-frequency counts never authorize it.

This corpus has a favorable shared prefix and 1% matching source rows. Fewer
scans alone were slower: the rejected variants document that limitation.
Without a provable selective shared filter, large predicate arrays can still
cost more than smaller queries. Byte/AST budgets are explicit tuning controls,
not a universal speed guarantee. The unsafe historical Hyperscan path is only
an ASCII performance reference; it is not restored to production.

The final queries are 199,831 and 157,836 bytes. Native parsed AST sizes were
39,199 and 30,943 nodes, below conservative estimates 44,065 and 34,865 and the
45,000-node generator budget. Those limits concern parsed ASTs; engine expanded
AST and execution limits remain separate.

## Insert-triggered watchlists

These runs time insertion of the million source rows through an actual
materialized view. Reading and validating the resulting hit table is untimed.
A completed sample over 60 seconds suppresses additional repetitions; it is a
single observation, not a distribution estimate. Timeouts have no median.

| Strategy | Samples | Seconds | Input rows/s | Peak query MiB |
|---|---:|---:|---:|---:|
| Exhaustive exact matching | 0 | >300, timeout | — | — |
| Previous lexical single needle | 0 | >300, timeout | — | — |
| Three ranked needles joined with AND | 1 | 243.792 | 4,102 | 2116.9 |
| Three file-frequency needles joined with AND | 1 | 178.894 | 5,590 | 2117.4 |
| **Sequential ranked-needle filters, final default** | **1** | **106.916** | **9,353** | **548.3** |
| **Sequential file-frequency filters, final optional ranking** | **1** | **83.037** | **12,043** | **547.2** |
| Historical Hyperscan, ASCII baseline | 3 | 5.583 | 179,100 | 14.6 |

The final SQL filters indicator indices through needle 1, then 2, then 3, then
exact `match()`. Later stages receive only surviving indices. It retains the
regex/view binding check and three scalar needle arrays. It does not depend on
AND short-circuiting to avoid constructing later-stage inputs for rejected
indices. Updates to watchlist rows do not require regenerating the MV.

The lexical needle admits all **1,000,000,000** row × indicator pairs. Both
three-needle selections admit **10,090**, or **0.001009%**, leaving 90 false
candidates for exact matching. These are independently queried logical
precheck counts. The frequency AND and staged runs each recorded 10,000 regex
cache misses plus 90 hits, consistent with those candidates; the improvement
is not evidence that staging reduced regex-cache accesses further. The lower
peak allocation and elapsed time are measured. Even the improved safe path
remains expensive for ingestion at this indicator count.

Frequencies came from 100 real Parquet files, each with 10,000 rows of this same
corpus, indexed and exported through `wc-lake`. The CSV has 1,360 grams and no
unindexed values. Counts such as `.ex` = 100 files versus `l00` = 10 files are
observed postings, not invented selectivity weights. Exported metadata,
validation, file hashes and the generator are retained alongside these results.

## Evidence and reproduction

Every completed variant checked the full sorted pair list against the known
oracle, not just a hit count. Canonical pair SHA-256:
`d6dbb4eafa8437a4b276f6cf08e28010e025567ff60a31ba5ed14dbd91d1724c`.
The additional all-indicator sweep canary checks a true positive for every
indicator, including those outside the first 1% of the timed source corpus.

[summary.json](summary.json) retains numeric results; individual JSONL files
retain samples, query IDs, query-log counters, settings and errors.
[run-status.json](run-status.json) records the original suite's order and cleanup,
including both timeout failures. Each failed query was canceled/finished and its
owned database removed before the next variant. Extra prototype runs are
identified by their separate logs and specification hashes.

[specs/index.json](specs/index.json) maps each measured generator hash to its
compressed **exact SQL specification**, including rejected variants. The hash
contract and original file hashes are explicit. Decompress the selected file
and run `libwildcard/tests/benchmarks/sweeps.py` with the strategy named in its
sample records. The original `spec.json.gz` predates the final code; current
`sweep_benchmark_gen` emits the promoted forms and frozen legacy baselines.
[Final production specification](spec-final-production.json.gz) and
[nine SQL identity checks](production-sql-identity.json) tie the committed
generator to the exact measured forms.
Early metadata `diagnostics` describe that specification's primary generated
plan; timed strategy/scan counts come from the sample records.

Task-owned timed suites ran sequentially, with no simultaneous task builds or
tests. Other host work, CPU scheduling and filesystem caches were uncontrolled.
Query and query-condition caches were disabled. Ordered small samples and this
synthetic distribution limit generalization. See the
[reproduction guide](../../../../../libwildcard/tests/benchmarks/README.md) and
[Elasticsearch comparison](../elastic/README.md); their different index and
query contracts matter.
