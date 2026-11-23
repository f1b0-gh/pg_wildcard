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
outer workspace currently pins the published `antalya-rs` client to `=0.1.0`; see
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
| Elasticsearch wildcard, 1 search at a time | 4,171.747 | 5,557.454 |
| Elasticsearch wildcard, up to 4 concurrent searches | 1,478.538 | 2,126.603 |

Elasticsearch's `_msearch` parameter `max_concurrent_searches` limits how many
individual rule searches can run at once within each batch; it is not a thread
count ([API definition](https://www.elastic.co/docs/api/doc/elasticsearch/operation/operation-msearch)).
The native, direct-client and PostgreSQL routes use one combined query, with
Antalya configured for `max_threads=4`. Each route runs one complete sweep at a
time; the PG route is not a four-client test or a single-threaded search control.

**Elasticsearch with four concurrent searches is faster in both fixtures.**
This compares latency at fixed data-engine CPU limits. It does not establish
which approach scales better across cores: that would require runs at different
CPU limits with corresponding parallelism settings.

### Methodology and limits

- **Setup:** Antalya 26.8.34.20001, Elasticsearch 9.5.6 and PostgreSQL 18.6.
  Each data-engine container was configured with four CPUs and an 8 GiB limit;
  Elasticsearch's heap was set to 4 GiB.
- **Queries:** identical documents and rules, with every result pair checked.
  Native/direct/PG execute one query against the same byte-indexed source.
  Elasticsearch batches 1,000 per-rule searches against a `wildcard` field
  using `_msearch`.
- **Sampling:** one excluded warmup, then five repetitions in rotating route
  order, after loading and compaction. Host activity and filesystem caches were
  uncontrolled.
- **Timing:** PostgreSQL backend measurements use `EXPLAIN ANALYZE` with row
  timing off; full COPY includes delivery over TCP. These PG queries consume
  computed match pairs without local `wc_*` rechecks. Subtracting route medians
  does not isolate client or FDW overhead.
- **Caches:** native/direct and Elasticsearch query caches were disabled. PG
  inherited the fixture settings; its profiles recorded condition-cache misses
  and no hits.

The table excludes ingestion and index construction. See the
[ingestion controls](tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/cost/report.md)
for those measurements, and the
[shared](tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/routes/full-shared/report.md)
and [unrelated](tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/routes/full-unrelated/report.md)
reports for individual timings, ranges and detailed settings.

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
Arrow Zstd measurements used a development build and showed both wins and
regressions; they do not change the Elasticsearch comparison above.

That opt-in compression support is now published in
[`antalya-rs` 0.1.1](https://crates.io/crates/antalya-rs/0.1.1), with uncompressed
Arrow remaining the default. pg_wildcard still pins `=0.1.0` and does not yet
expose the Zstd option through its FDW. The direct-client and PostgreSQL timings
above were measured using 0.1.0; the separate compression results remain
measurements of a development build.

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
