<!-- SPDX-License-Identifier: Apache-2.0 -->
# Elasticsearch 9.5.6: 1M URLs × 1,000 literal indicators

Complete raw evidence and the original hash manifests are preserved in the
[checksummed release archive](../../../ARTIFACTS.md). From the repository root,
run `python3 tools/verify_benchmark_artifacts.py` to verify this historical run.


Every sweep returned the same **10,000 indicator–row pairs**, checking all row IDs
per indicator. Three ordered repeats, seconds:

| Field | Repeat 1 | Repeat 2 | Repeat 3 | Median |
|---|---:|---:|---:|---:|
| `wildcard` | 3.402 | 1.575 | 1.218 | **1.575** |
| `keyword` | 10.138 | 7.298 | 6.982 | **7.298** |

The wildcard field's median was 4.63× faster in this comparison. Each sweep sends
1,000 queries `*evilNNNN.example*`, bundled in ten `_msearch` requests with at most
four searches executing concurrently. It retrieves IDs with exact hit counts;
wall time includes HTTP JSON, decoding and result validation. Server `took` is
saved per indicator. Its sum overlaps at concurrency four and is **not** elapsed
time. Native Elasticsearch HTTP JSON is a different route from Antalya Arrow
and PostgreSQL FDW.

Canonical pair SHA256 (sorted `row_id TAB indicator_id TAB raw LF`):
`d6dbb4eafa8437a4b276f6cf08e28010e025567ff60a31ba5ed14dbd91d1724c`.
The corpus matches the sweep fixture: every hundredth row contains
`https://evil{(id//100)%1000:04}.example/path`; other values are
`https://benign.example/path/{id}`.

The separate single bool-union count query returned **10,000 distinct rows** in
0.627s (`wildcard`, server 618ms) and 10.901s (`keyword`, server 10,894ms).
It does not return indicator–row pairs or materialize hits; do not compare these
times as if they were the sweep workload.

| Index construction | Seconds | Primary store bytes | MiB |
|---|---:|---:|---:|
| `wildcard` | 39.176 | 67,567,646 | 64.44 |
| `keyword` | 35.993 | 25,402,822 | 24.23 |

Index construction includes client corpus generation, sequential HTTP bulk,
refresh and flush. These single setup measurements could overlap builds; they
are **not watchlist/MV ingestion timings**. Each separate index has one shard,
no replicas and retained `id,v` source; source JSON totals 53,728,892 bytes.
Both had two segments after setup. No force merge was requested. The first
wildcard load was repeated after a stats-collection error; the retry is retained
in the raw log. Shared URL prefixes can make wildcard storage larger, as the
[public field documentation](https://www.elastic.co/docs/reference/elasticsearch/mapping-reference/keyword#wildcard-field-type)
notes.

The isolated engine had four CPU quota units, 4 GiB container memory and a 2 GiB
JVM heap. Timed query runs had an exclusive slot among this task's benchmarks;
other host work is not controlled. Request/query caches were disabled; OS cache
and JIT state were not reset. The descending repeats show warm-up effects; no
cold-cache claim is made. Settings, image digest, machine information and source
hash are saved in `settings-queries.json`. Setup settings are separate. Disk
watermarks were changed only on this fixture to 10/5/2 GiB free after the default
90% high watermark refused allocation despite approximately 44 GiB free.

Semantic probes cover empty strings, newline, backslash, NUL and case folding.
An empty wildcard expression matched no values; `*` included the empty value.
In these probes `case_insensitive=true` folded ASCII, but did not fold é/É,
k/K/K, σ/Σ/ς or ss/ß. This establishes **literal ASCII workload equivalence**,
not general RE2/Lucene-regexp parity. Invalid UTF-8 byte values cannot be
represented in this JSON string corpus. No percolator comparison was performed.

Reproduce from the repository root using an owned fixture's private state:

```sh
python3 tests/benchmarks/elasticsearch.py --state .local/perf-review/elasticsearch.json \
  --rows 1000000 --indicators 1000 --repeats 3 \
  --outresults tests/benchmarks/results/2026-11-22/elastic --phase setup
# Reserve an uncontended benchmark slot before this command:
python3 tests/benchmarks/elasticsearch.py --state .local/perf-review/elasticsearch.json \
  --rows 1000000 --indicators 1000 --repeats 3 \
  --outresults tests/benchmarks/results/2026-11-22/elastic --phase queries --union
```

`raw.jsonl` retains per-indicator IDs, counts, server times and setup stats;
`summary.json` contains the compact measured results. Only public documentation
was consulted: [wildcard query](https://www.elastic.co/docs/reference/query-languages/query-dsl/query-dsl-wildcard-query),
[regexp query](https://www.elastic.co/docs/reference/query-languages/query-dsl/query-dsl-regexp-query),
[msearch](https://www.elastic.co/docs/api/doc/elasticsearch/operation/operation-msearch),
[disk watermarks](https://www.elastic.co/docs/reference/elasticsearch/configuration-reference/cluster-level-shard-allocation-routing-settings).
