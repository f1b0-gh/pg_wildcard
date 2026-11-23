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

The extension targets PostgreSQL **18.6** with pgrx **0.19.4**. The
current checkout uses `antalya-rs =0.1.1`. The published `pg_wildcard 0.2.1`
archive still uses `antalya-rs =0.1.0`; the qualified client update is not yet
released. See the [build guide](docs/build.md) for prerequisites and installation.

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

The **2026-11-23 requalification** uses `libwildcard 0.2.1` and the current
`pg_wildcard 0.2.1` sources with `antalya-rs 0.1.1`. These are local-machine
observations, with uncontrolled host activity and filesystem caches. They have
not been independently reproduced in a controlled benchmark environment.

### Complete sweeps

**One million documents, 1,000 wildcard rules, 10,000 exact row/indicator/view
pairs.** Every route returns the same pairs. URL-shaped fields contain either
shared literals (`*evil0000.example*`, `*evil0001.example*`) or unrelated literals
(`*e220a8397b1dcdaf*`, etc.). These are substring searches.

Six measured repetitions follow one excluded warmup, with rotating route order.
Times are **median [minimum, maximum] milliseconds**.

| Route | Shared literals | Unrelated literals |
|---|---:|---:|
| Native Antalya HTTP, byte index | 1,092.595 [1,024.274, 1,271.077] | 1,819.531 [1,261.818, 2,339.980] |
| Direct `antalya-rs 0.1.1` | 1,931.827 [1,885.563, 2,052.146] | 2,728.737 [1,869.048, 3,040.210] |
| PostgreSQL EXPLAIN request through FDW | 1,977.298 [1,845.909, 2,044.811] | 2,626.485 [1,790.449, 2,775.892] |
| Complete PostgreSQL binary COPY through FDW | 1,721.042 [1,650.506, 1,899.233] | 2,451.364 [1,979.624, 3,352.971] |
| Elasticsearch wildcard, 1 concurrent search | 2,467.283 [1,913.265, 2,782.272] | 2,262.281 [1,962.713, 2,926.844] |
| Elasticsearch wildcard, up to 4 concurrent searches | 646.618 [488.583, 761.040] | 1,167.470 [465.480, 1,244.821] |

Elasticsearch's four-search mode has the lowest median in both fixtures.
Its `_msearch` concurrency counts rule searches; Antalya executes one combined
query with `max_threads=4`. These settings do not establish equivalent CPU use
or multicore scaling. The generated pair queries have no local `wc_*` recheck;
subtracting route medians does not isolate FDW overhead.

### Paired FDW follow-up

A separate comparison holds `antalya-rs 0.1.1` fixed and measures the FDW
optimizations against their predecessor. Six rounds balance all orders of the
three variants used to separate the client update from the FDW changes.
Each cell is the **median paired latency reduction % [minimum, maximum]**;
positive means faster, negative means slower. Ranges are observed extrema,
not confidence intervals.

| Workload | PostgreSQL executor | Complete binary COPY |
|---|---:|---:|
| Narrow | 5.47 [-8.26, 27.26] | -0.68 [-32.21, 27.41] |
| Short text | -3.24 [-18.77, 4.49] | -4.96 [-15.55, 43.17] |
| Wide, 1 KiB | -15.50 [-33.54, 6.99] | -1.26 [-7.50, 5.84] |
| Broad recheck | 2.36 [-83.83, 15.44] | 3.98 [-25.96, 30.73] |

**No consistent FDW optimization gain is demonstrated.** Every paired range
crosses zero, and some workloads have slower medians. Broad recheck retains
the local `wc_match` filter over one million rows; the unchanged direct client
serves as a variability control, not evidence of an FDW improvement.

### Qualification and scope

The retained stack is PostgreSQL 18.6 / pgrx 0.19.4, Antalya 26.8.34.20001 and
Elasticsearch 9.5.6. Both data engines have four CPUs and 8 GiB; Elasticsearch
has a 4 GiB heap. Arrow is uncompressed. Sweep and FDW workloads use different
query settings and timing boundaries; their latencies are not interchangeable.

Correctness checks passed: 212 all-feature library tests, 38 transport tests,
50 FDW harness tests, 86 PostgreSQL tests, native integration, differential
seeds and Iceberg/Puffin coverage. Package and license checks passed. One existing
manual library benchmark remains ignored; other PostgreSQL majors were not
requalified.

See the [benchmark guide](tests/benchmarks/README.md#current-requalification)
for settings, client-update comparisons and evidence availability. Earlier
[sweeps](tests/benchmarks/results/2026-11-22/perf-plan/byte-dispatch/) and
[FDW flamegraphs](tests/benchmarks/results/2026-11-23/fdw-overhead/) remain
historical results; their timings and gains do not describe this candidate.

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
