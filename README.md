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
libwildcard = { version = "0.2.2", features = ["hot"] }
```

```rust
use libwildcard::core::{Matcher, View};

let matcher = Matcher::wildcard("*/wp-admin/*.php", View::Decoded)?;
assert!(matcher.is_match(b"GET /%77p-ADMIN/a.PHP"));
```

Both [`libwildcard`](https://crates.io/crates/libwildcard) and
[`pg_wildcard`](https://crates.io/crates/pg_wildcard) are published as **0.2.2**.

## PostgreSQL

The extension targets PostgreSQL **18.6** with pgrx **0.19.4**. Release 0.2.2
uses the qualified `antalya-rs =0.1.1` client. See the [build guide](docs/build.md) for prerequisites and installation.

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

The **2026-11-24 comparison** remeasures all eight routes together using the
qualified source candidate for 0.2.2 (before the version-only release bump). These are local shared-machine observations; unrelated host
activity, text-index caches and filesystem caches remain uncontrolled. They have
not been independently reproduced in a controlled benchmark environment.

### Complete sweeps

**One million documents, 1,000 wildcard rules, 10,000 exact row/indicator/view
pairs.** URL-shaped fields contain either shared literals (`*evil0000.example*`,
`*evil0001.example*`) or unrelated literals (`*e220a8397b1dcdaf*`, etc.). These are
substring searches. The source, rules and original queries match the frozen
reference; no historical medians are mixed into this table.

Seven measured repetitions follow one excluded warmup, with rotating route order.
Times are **median [minimum, maximum] milliseconds**.

| Route | Shared literals | Unrelated literals |
|---|---:|---:|
| Native Antalya HTTP, byte index, 1 query / 4 threads | 2,007.328 [1,660.970, 2,147.936] | 1,997.496 [1,687.351, 2,205.600] |
| Native Antalya, 4 indexed batches / 1 thread each (opt-in) | 940.077 [847.289, 1,127.050] | 1,466.480 [1,289.097, 1,804.884] |
| Native Antalya, selective staging + 4 batches (opt-in) | 625.592 [526.265, 1,227.035] | 724.931 [634.044, 1,076.017] |
| Direct `antalya-rs 0.1.1` | 2,768.350 [2,051.879, 2,922.438] | 2,457.666 [2,013.494, 3,102.955] |
| PostgreSQL EXPLAIN request through FDW | 2,764.748 [2,649.116, 3,028.674] | 2,836.696 [2,641.858, 2,881.849] |
| Complete PostgreSQL binary COPY through FDW | 2,385.819 [2,293.513, 3,134.124] | 2,430.472 [2,290.985, 2,790.017] |
| Elasticsearch wildcard, 1 concurrent search | 3,529.313 [2,364.557, 5,690.472] | 3,920.632 [2,562.465, 4,955.963] |
| Elasticsearch wildcard, up to 4 concurrent searches | 1,116.308 [786.087, 1,699.909] | 990.876 [867.078, 1,153.049] |

Selective staging has the lowest median in both fixtures in this run. Against
four indexed batches, its median latency falls **33.5% / 50.6%**, with **6/7 and
7/7** corresponding rounds faster. The clock includes candidate preflight, fresh
materialization, population verification, exact checks and synchronous cleanup.
One shared-literal round is slower; the observed ranges are not confidence intervals.

The new [staging API](libwildcard/INTEGRATION.md#opt-in-selective-staging)
is opt-in and available since 0.2.2. It
shares a small candidate set and removes the redundant byte-index predicate on
the Memory table while retaining exact checks. Dense controls refuse staging
before materialization and pay the preflight cost. This is a conditional strategy,
not a default routing change.

PostgreSQL rows use the existing FDW path and the original combined query.
Elasticsearch concurrency counts rule searches; the batched native routes permit
four one-thread queries. These settings do not establish identical CPU use.
Generated pair queries have no local `wc_*` recheck, and subtracting route medians
does not isolate FDW overhead. The separate
[2026-11-23 paired FDW study](tests/benchmarks/README.md#paired-fdw-protocol)
did not demonstrate a consistent FDW optimization gain.

### Qualification and scope

The stack is PostgreSQL 18.6 / pgrx 0.19.4, Antalya 26.8.34.20001 and Elasticsearch
9.5.6. Both data engines have four CPUs and 8 GiB; Elasticsearch has a 4 GiB heap.
Arrow is uncompressed. PostgreSQL EXPLAIN request and full binary COPY clocks
remain distinct. The FDW binary is unchanged and was qualified separately on
2026-11-23; this run verifies its complete sweep route.

All **144 million-document sweeps**, including warmups and dense controls, passed
their result checks with no censored samples. Returning routes check the complete
pair multiset; PostgreSQL EXPLAIN consumes the same SQL validated by full COPY.
The library passed 217 all-feature tests, three differential seeds on each of
chDB and native Antalya, formatting, strict Clippy, package and license checks.
The benchmark harness passed 74 portable tests and the native cancellation/isolation
regression. One existing manual library benchmark remains ignored.

See the [benchmark guide](tests/benchmarks/README.md#current-requalification) for
settings, the mechanism, counterexamples and evidence availability. Raw results
and independent recalculation remain local; the older downloadable evidence
archives do not verify this new table.

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
