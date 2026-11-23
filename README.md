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

Add the published library to your application, selecting only the features you need:

```toml
[dependencies]
libwildcard = { version = "0.2.1", features = ["hot"] }
```

```rust
use libwildcard::core::{Matcher, View};

let matcher = Matcher::wildcard("*/wp-admin/*.php", View::Decoded)?;
assert!(matcher.is_match(b"GET /%77p-ADMIN/a.PHP"));
```

Both [`libwildcard`](https://crates.io/crates/libwildcard) and
[`pg_wildcard`](https://crates.io/crates/pg_wildcard) are published as **0.2.1**.

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

This comparison searches **one million documents with 1,000 wildcard rules**,
returning the same **10,000 exact row/indicator/view pairs** on every route.
The URL-shaped fields contain either shared literals (`*evil0000.example*`,
`*evil0001.example*`) or unrelated literals (`*e220a8397b1dcdaf*`, etc.).
These are substring searches, not exact hash lookups.

Historical matched comparison, before the 0.2.1 FDW changes:
median complete-request latency in milliseconds (2026-11-22).

| Route | Shared structure | Unrelated literals |
|---|---:|---:|
| Native Antalya HTTP, refined byte index | 1,982.730 | 2,730.355 |
| Direct published `antalya-rs` 0.1.0 | 2,657.454 | 3,672.923 |
| PostgreSQL backend request through FDW | 2,742.254 | 3,702.678 |
| Full PostgreSQL binary COPY through FDW | 3,119.934 | 3,458.408 |
| Elasticsearch wildcard, 1 search at a time | 4,171.747 | 5,557.454 |
| Elasticsearch wildcard, up to 4 concurrent searches | 1,478.538 | 2,126.603 |

**Elasticsearch's four-search mode is faster in both fixtures.** Its
[`_msearch` concurrency](https://www.elastic.co/docs/api/doc/elasticsearch/operation/operation-msearch)
counts concurrent rule searches. Antalya executes one combined query with
`max_threads=4`. This comparison does not establish multicore scaling.

The **0.2.1 FDW follow-up** used separate million-row broad-recheck queries:
paired median reductions were **15% backend / 25% complete COPY**, with mixed
results elsewhere. Estimated non-matcher CPU fell **29%**, below the 50% target.
These measurements do not update the Elastic comparison above.
[Results, ranges and before/after flamegraphs](tests/benchmarks/results/2026-11-23/fdw-overhead/).

### Methodology and limits

**These are local-machine measurements, not yet a reproducible benchmark.**
They have not been independently reproduced or validated in a controlled
benchmark environment.

- **Setup:** Antalya 26.8.34.20001, Elasticsearch 9.5.6, PostgreSQL 18.6;
  four CPUs and 8 GiB per data engine, with a 4 GiB Elasticsearch heap.
  Direct/PG use `antalya-rs` 0.1.0 and uncompressed Arrow.
- **Sampling:** one excluded warmup, five repetitions, rotating route order,
  one sweep at a time. Host activity and filesystem caches were uncontrolled.
- **Timing:** backend requests use `EXPLAIN ANALYZE` with row timing off;
  COPY includes TCP delivery. These computed-pair queries have no local
  `wc_*` recheck. Subtracting route medians does not isolate FDW overhead.
- **Scope:** wildcard fields and opt-in byte indexing. Ingestion and index
  construction are measured separately; keyword and hash lookups are excluded.

See the [detailed results and methodology](tests/benchmarks/README.md#measured-performance)
for SQL, exact-result checks, cache settings, sample ranges, ingestion costs and
scripts for rerunning the experiments.

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
