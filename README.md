<!-- SPDX-License-Identifier: Apache-2.0 -->
# pg_wildcard

**Wildcard and regex search for telemetry.**

pg_wildcard is an Apache-2.0 Rust toolkit for searching URLs, domains, user
agents, command lines and paths. It combines a standalone library, a PostgreSQL
extension, ClickHouse/Antalya SQL generation and Iceberg file pruning.

## Why this project exists

Security investigations often start with a fragment rather than a complete
value: part of a command line, an encoded URL, a suspicious domain or a path.
Tokenizing or permanently normalizing these strings can hide distinctions that
matter as evidence. pg_wildcard keeps the original value and searches explicit
derived views when decoding, case folding or path handling is useful.

The goal is to make those searches available through SQL across hot telemetry
and historical lake data. PostgreSQL provides the control plane; ClickHouse or
Antalya executes telemetry queries; compact Puffin indexes help exclude Iceberg
files that cannot match. Applications can also use the Rust library directly,
without PostgreSQL or a network client.

This is a search toolkit, not a replacement for an entire search platform or
user interface. Wildcards are familiar, but regex semantics follow ClickHouse's
RE2 dialect rather than Lucene. Performance depends on selectivity, layout,
index coverage and execution overhead; the project makes no general claim to
outperform Elasticsearch.

## Components

| Component | Purpose |
|---|---|
| [`libwildcard`](libwildcard/README.md) | Canonical matching, planning, SQL generation, Puffin codecs and lake indexing |
| `pg_wildcard` | PostgreSQL `wc_*` functions, planner support and `wildcard_fdw` |
| `wc-lake` | Index Parquet files, refresh sidecars, generate lake queries and export index data |
| `crates/wildcard-*` | Compatibility re-exports; the lake target builds the canonical CLI |

```text
PostgreSQL / application → ClickHouse or Antalya → hot telemetry / Iceberg
       libwildcard       → exact predicates     → candidate files + recheck
           wc-lake       → Puffin indexes       → conservative lake pruning
```

The library supports `raw`, `lower`, `decoded`, `revdomain` and `winpath` views.
Indexes select candidates; exact predicates determine matches. Lake consumers
must preserve snapshot, file and field bindings. See the [semantics](docs/semantics.md)
and [lake coverage rules](docs/lake.md) before enabling pruning.

## Start with the library

Rust **1.99.1** is pinned in this repository. The standalone workspace needs no
PostgreSQL, companion client or sibling checkout:

```sh
cargo test --locked --manifest-path libwildcard/Cargo.toml
cargo test --locked --manifest-path libwildcard/Cargo.toml --all-features
cargo build --locked --manifest-path libwildcard/Cargo.toml --features cold --bin wc-lake
```

Add it to an application by source path, selecting only the features you need:

```toml
[dependencies]
libwildcard = { path = "path/to/pg_wildcard/libwildcard", features = ["hot"] }
```

```rust
use libwildcard::core::{Matcher, View};

let matcher = Matcher::wildcard("*/wp-admin/*.php", View::Decoded)?;
assert!(matcher.is_match(b"GET /%77p-ADMIN/a.PHP"));
```

Use the library's source package for an independent distribution. No crates.io
release is assumed by these instructions.

## PostgreSQL

The extension targets PostgreSQL **18.6** with pgrx **0.19.4**. Building the
outer workspace uses the published `antalya-rs` 0.1.0 client from crates.io; see
the [build guide](docs/build.md) for prerequisites and CI configuration.

After installing and creating the extension:

```sql
SELECT wc_match('GET /%77p-ADMIN/a.PHP', '*/wp-admin/*.php', 'decoded');
SELECT wc_ch_predicate('url', '*/wp-admin/*.php', 'wildcard', 'decoded', true);
SELECT * FROM wc_indicator('hxxps://Login.Evil[.]com/');
```

`wildcard_fdw` exposes remote telemetry as foreign tables, pushes supported
predicates down and preserves local rechecks. Its transport uses the companion
client's completion contract; generic ClickHouse HTTP availability alone does
not establish FDW compatibility. See the [FDW guide](docs/fdw.md).

## Measured performance

The latest indexed comparison (2026-11-22) searches **one million documents with
1,000 case-sensitive wildcard rules**, returning the same **10,000 exact
row/indicator/view pairs** on every route. These are substring searches in
URL-shaped wildcard fields:

- **Shared structure:** `*evil0000.example*`, `*evil0001.example*`, etc.
- **Unrelated literals:** `*e220a8397b1dcdaf*`, `*6e789e6aa1b965f4*`, etc., matching
  independent synthetic URL labels. These are not exact hash lookups.

Median complete-request latency in milliseconds, with all results consumed:

| Route | Shared structure (ms) | Unrelated literals (ms) |
|---|---:|---:|
| Native Antalya HTTP, refined byte index | 1,982.730 | 2,730.355 |
| Direct published `antalya-rs` 0.1.0 | 2,657.454 | 3,672.923 |
| PostgreSQL backend request through FDW | 2,742.254 | 3,702.678 |
| Full PostgreSQL binary COPY through FDW | 3,119.934 | 3,458.408 |
| Elasticsearch wildcard, `_msearch` concurrency 1 | 4,171.747 | 5,557.454 |
| Elasticsearch wildcard, `_msearch` concurrency 4 | 1,478.538 | 2,126.603 |

**Elasticsearch at concurrency four remains faster in both fixtures.** The native,
direct-client and PostgreSQL routes measure different operations and must remain
separate. These two synthetic workloads do not establish a general engine ranking.

### Methodology and limits

- **Engines and resources:** Antalya 26.8.34.20001 and Elasticsearch 9.5.6 each
  have four CPUs and an 8 GiB container limit; Elasticsearch has a 4 GiB heap.
  PostgreSQL 18.6 and the direct client add resources outside those engine limits.
- **Matching and indexes:** identical raw documents and rules, with complete
  result-pair checks. Native/direct/PG share the same byte-indexed source and
  execute one source query per sweep. Both Elasticsearch routes use the same
  `wildcard` field index, with 1,000 per-rule searches batched in `_msearch`.
  This measures neither BM25 scoring nor percolation.
- **Sampling:** one excluded warmup followed by five measured repetitions in
  rotating route order, after ingestion and compaction. Shared-host activity
  and filesystem caches were uncontrolled; these are warm observations.
  [Shared](tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/routes/full-shared/report.md)
  and [unrelated](tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/routes/full-unrelated/report.md)
  reports retain ranges and individual observations.
- **Route boundaries:** the direct route uses published `antalya-rs`, not raw
  HTTP. PG backend timing uses `EXPLAIN ANALYZE` with per-row timing off; full
  COPY includes delivery over TCP. These PG queries consume generated exact
  pairs without additional local `wc_*` rechecks. Transport, server and backend
  clocks overlap; subtracting route medians does not isolate overhead. Separate
  [FDW profiles and flamegraphs](tests/benchmarks/results/2026-11-22/perf-plan/fdw-conversion/)
  cover broad local rechecks and conversion.
- **Query caches:** native/direct explicitly disable the condition cache;
  Elasticsearch disables request/query caches. PG inherits the fixture policy:
  its saved profiles record two condition-cache misses per query and no recorded
  hits. The reports correct the original metadata's blanket cache-disable claim.
- **Indexing cost:** the table excludes index construction and ingestion. Separate
  [matched-fsync ingestion controls](tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/cost/report.md)
  use identical 2,000-document batches, 900,000 initial plus 100,000 incremental
  documents, and confirmed search visibility. They are single observations,
  not throughput medians; earlier unequal-durability setup times cannot rank
  ingestion speed.

The byte-index strategy is opt-in. Requiring up to three necessary fragments
together per rule reduced unrelated-case candidates from 921,471 to 13,407,
preserving all 10,000 exact pairs. Forced-index checks and saved `PREWHERE` plans
establish index use; logical `read_rows` alone does not measure physical I/O.
The refined filter regressed on the small smoke fixture, so a speedup is not
guaranteed. Keyword, typed and hash fields are outside this wildcard comparison.

See the [full evidence and reproduction guide](tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/)
for SQL, settings, raw profiles, corpus/result hashes, source provenance and an
offline report verifier, or the [benchmark harness](tests/benchmarks/README.md)
to run new measurements.

Separate [FDW tuning results](tests/benchmarks/results/2026-11-22/perf-plan/fdw-tuning/)
cover block sizes, selective lake completion and adaptive-cache churn. They keep
direct published antalya-rs, backend execution and full COPY separate. The cache
fix raised hot-preference hits from 80% to 100% in five interleaved hot/new-key
trials, removing 16 repeated probes per round without extending expiry. The
upstream Arrow Zstd experiment is an opt-in modified-client result, with both
wins and regressions; it does not change the Elasticsearch comparison above.

## Documentation

- [Build and installation](docs/build.md)
- [Architecture and boundaries](docs/architecture.md)
- [Matching semantics](docs/semantics.md)
- [SQL API](docs/sql-api.md)
- [Lake indexing and CLI](docs/lake.md)
- [Foreign data wrapper](docs/fdw.md)
- [Diagnostics and migrations](docs/operations.md)
- [Testing and compatibility](docs/testing.md)
- [Contribution rules](AGENTS.md)

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
