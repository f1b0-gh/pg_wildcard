<!-- SPDX-License-Identifier: Apache-2.0 -->
# Indexed byte-fragment dispatch

The opt-in raw-view dispatch uses a bounded byte-fragment text index to narrow
source rows, then routes each candidate to possible indicators and checks exact
regex matches. The default sweep strategy and regex engine are unchanged. Only
admitted wildcard fields and explicitly eligible wildcard observables belong in
this path; hashes and ordinary keyword/typed fields remain excluded.

## What these fixtures mean

Both fixtures contain one million raw documents and 1,000 distinct case-sensitive
rules, returning the same 10,000 row/indicator/view pairs on every compared route.
They test substring wildcards within URL-shaped text:

- Shared structure: `*evil0000.example*`, `*evil0001.example*`, and so on, matching
  values such as `https://evil0000.example/path`. Most rule text is common.
- Unrelated literals: `*e220a8397b1dcdaf*`, `*6e789e6aa1b965f4*`, and so on, matching
  synthetic URL labels. These are wildcard substring searches in a declared
  wildcard field, not exact hash lookups. No common long literal connects all rules.

These distributions do not represent all wildcard workloads. Native-only byte,
Unicode, long-value, alias and index-lifecycle controls test correctness beyond
this shared engine subset; they do not supply equivalent Elastic timings.

## Index use and selectivity

The first gate used one necessary fragment per rule, ORed across all rules. It
used the index but admitted 921,471 of one million unrelated-case rows. The
refined gate requires up to three proven fragments together for each rule. It
admits 13,407 rows in that case and preserves all 10,000 exact output pairs.
Shared-case candidates decrease from 269,281 to 10,000. Both gates use the same
physical index; this improvement is in how the query uses it.

Saved forced-index checks, `EXPLAIN PLAN actions=1` virtual text-index filters in
`PREWHERE`, and query-log counters establish index use. Logical `read_rows` can
still be one million; it is not proof of an unindexed scan. Logical `read_bytes`
includes generated filter columns and must not be reported as physical I/O.
[ClickHouse's text-index documentation](https://clickhouse.com/docs/reference/engines/table-engines/mergetree-family/textindexes)
describes the virtual-column optimization; the saved plans establish behavior
on the actual pinned engine, independently of current upstream documentation.

Index extraction covers at most 8,192 raw bytes. Overlong values carry an overflow
token and remain eligible for exact checking. Hex encoding preserves arbitrary
bytes; half-byte windows can add candidates. SQL/AST budgets can widen the gate
or return the existing sweep strategy. This is an opt-in index, not an automatic
migration or a guarantee of useful pruning for every rule set.

## Query measurements

Native-only comparison, five rotating repetitions after warmup, complete result
consumption and identical pair checks (median milliseconds):

| Fixture | Existing indexed sweeps | One-fragment gate | Required-fragment gate |
|---|---:|---:|---:|
| Shared structure | 3,535.383 | 1,820.368 | 1,629.951 |
| Unrelated literals | 26,188.486 | 9,848.770 | 2,072.031 |

See [native full-scale report](native/full/report.md) and
[1% smoke report](native/smoke/report.md). The refined gate is slower than the
coarse gate on the small smoke fixture: larger SQL and filter evaluation have a
cost. No universal speedup follows from the full-scale improvement.

A separate equally resourced data-engine comparison uses the same raw documents
and exact pair contract. Both engines have four CPUs and 8 GiB container limits;
Elastic has a 4 GiB heap. Five rotating measured repetitions follow one warmup:

| Complete-request route | Shared ms | Unrelated ms |
|---|---:|---:|
| Native HTTP, refined byte index | 1,982.730 | 2,730.355 |
| Direct published antalya-rs 0.1.0 | 2,657.454 | 3,672.923 |
| PostgreSQL backend request | 2,742.254 | 3,702.678 |
| Full PostgreSQL binary COPY | 3,119.934 | 3,458.408 |
| Elastic wildcard, msearch concurrency 1 | 4,171.747 | 5,557.454 |
| Elastic wildcard, msearch concurrency 4 | 1,478.538 | 2,126.603 |

See [shared route report](routes/full-shared/report.md) and
[unrelated route report](routes/full-unrelated/report.md) for ranges and raw data.
**Elastic with concurrency four remains faster in both cases.** Comparing only
the serial Elastic route would hide that result. Both Elastic routes use one
wildcard index. Native/direct/PG use one shared byte-indexed source and one source
query per sweep; Elastic performs 1,000 per-rule searches batched in `_msearch`.
No BM25 scoring or Elastic percolator workload is measured.

Direct and PostgreSQL add client/backend resources and work beyond the data
engine. Their timings must remain separate. The PG route consumes the generated
exact-pair query through the FDW; it does not add local `wc_*` rechecks. The older
[FDW conversion evidence](../fdw-conversion/README.md) contains the local-recheck
workload and CPU flamegraphs. Direct first-batch, decode, terminal-confirmation,
server, executor and full-delivery clocks overlap; do not subtract them into
invented exclusive stages or label first batch as network TTFB.

The original PG runs inherited the fixture cache policy: each recorded native
query has two query-condition-cache misses and no recorded hits. Native/direct
explicitly disable that cache; Elastic disables request/query caches. Original raw
metadata incorrectly claimed all query caches were disabled; the route reports
and their new counter summaries correct it without altering timings or raw logs.
This does not establish a cache-hit advantage or change the native/Elastic ranking.
The FDW rejects per-query cache overrides; the public collector now describes the
inherited PG policy explicitly rather than claiming uniform disablement.

## Indexed ingestion and storage

Query latency is measured after ingestion and compaction. Earlier route setup
costs used unequal durability defaults and are retained only as explicitly labeled
observations. They must not rank ingestion speed across engines. The separate
[fsync cost control](cost/report.md) enables native inserted-part and directory
fsync and Elastic request-translog fsync, saving and checking effective settings.
This aligns configured acknowledgment behavior; no hardware crash testing is
claimed. See [ClickHouse fsync settings](https://clickhouse.com/docs/reference/settings/merge-tree-settings/fsync)
and [Elastic translog settings](https://www.elastic.co/docs/reference/elasticsearch/index-settings/translog).

Both receive identical documents via 2,000-document HTTP batches, 90% initial and
10% incremental, maintaining the index on insert. Timers run through searchable
row-count confirmation, including Elastic refresh. Serialization and compaction
are reported separately. Source hashes confirm every final value. One build per
engine and fixture is a cost observation, not a throughput median or a statistical
speedup. These batch sizes do not establish maximum bulk-ingestion throughput.
Stored data-part bytes and Elastic index-store bytes include different complete
representations; neither is a comparable index-only size or whole-process memory.

## Reproduction and provenance

`sha256.json` covers every payload other than itself. `verify.py` checks exact
membership, hashes, gzip integrity, complete cohorts, pair/corpus digests and
settings, then regenerates reports offline in a temporary directory. Run from the
repository root:

```sh
python3 tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/verify.py
```

Post-review qualification preserves selected source fields that collide with hit
labels, checks both named and positional results, and keeps the legacy public
strategy enum source-compatible. All four measured benchmark specifications remain
JSON-identical after those fixes. Updated native/chDB lifecycle logs and source
provenance are included. A pre-timing attempt to override caches through FDW server
options was rejected by its allowlist; that diagnostic is retained and the supported
settings remain unchanged. Historical cache descriptions are corrected above.

Large JSON and SQL payloads use deterministic gzip. Exact executed measurement
scripts, specifications, settings, SQL, query profiles and source provenance are
included; credentials, private endpoints and process logs are excluded. The full
route runs used a private adapter whose executed collector is saved. The public
`compare_routes.py --layout byte_dispatch` replaces that adapter and has its own
six-route 1% correctness smoke, with fsync enabled. It does not retroactively
replace the recorded source of the full measurements.

To rerun, use the [benchmark instructions](../../../../README.md), cached pinned
engine images, fresh owned fixtures and a fresh output directory. Select Elastic's
8 GiB fixture explicitly. Generate `shared` and `unrelated` specifications at
1,000 indicators / one million rows, and run `compare_routes.py` with
`--layout byte_dispatch --ingest-durability fsync --repeats 5`, the direct published
client worker and an owned PostgreSQL fixture. The public layout defaults to all
six routes above. PostgreSQL inherits the native fixture cache policy; inspect its
query-log profiles rather than assuming the client overrides that policy. Save the actual resource and concurrency settings. The native
three-strategy runner and its coarse/refined specifications are also preserved.

No local builds or tests overlapped measurement. Shared-host activity, filesystem
cache state and runtime warming were uncontrolled; host load, query-cache settings,
measurement order and individual observations are retained. One short progress
probe and one durability audit occurred during the original unrelated Elastic
load; their durations are preserved with that default-setting run. No benchmark
results are inferred for unmeasured layouts, invalid-byte Elastic semantics,
watchlist ingestion, percolation or alternative regex engines.
