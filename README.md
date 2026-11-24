<!-- SPDX-License-Identifier: Apache-2.0 -->
# pg_wildcard

**Wildcard and regex search for telemetry, from hot storage to Iceberg.**

Search URLs, domains, user agents, command lines and paths with a standalone
[Rust library](libwildcard/README.md), [PostgreSQL functions and FDW](docs/fdw.md),
ClickHouse/Antalya SQL generation, and [Puffin lake indexes](docs/lake.md).
Raw values remain intact; explicit views handle decoding and normalization.
Pruning retains exact rechecks, and regex semantics follow ClickHouse/RE2.

## Quick start

The library needs no PostgreSQL or network client:

```toml
[dependencies]
libwildcard = { version = "0.2.2", features = ["hot"] }
```

```rust
use libwildcard::core::{Matcher, View};

let matcher = Matcher::wildcard("*/wp-admin/*.php", View::Decoded)?;
assert!(matcher.is_match(b"GET /%77p-ADMIN/a.PHP"));
```

See [library integration](libwildcard/INTEGRATION.md) for features and
[installation](docs/build.md) for the PostgreSQL extension and `wc-lake` CLI.
Both packages are at **0.2.2**; the extension uses `antalya-rs 0.1.1`.

## Measured performance

**2026-11-24: nine routes measured together** on **1M documents, 1,000 wildcard
rules and 10,000 exact row/indicator/view pairs**. URL-shaped corpora use shared
or unrelated literals. One warmup, then seven repetitions in rotating order;
values are **median [minimum, maximum] milliseconds**, rounded to whole ms.

Antalya 26.8.34.20001 and Elasticsearch 9.5.6 each have four CPUs and 8 GiB.
PostgreSQL is 18.6 / pgrx 0.19.4; both Rust routes use `antalya-rs 0.1.1`.
Rust timings include IPC and complete delivery. EXPLAIN measures backend
consumption; COPY includes delivery. Opt-in staging includes preflight,
materialization, exact checks and cleanup.

| Route | Shared literals | Unrelated literals |
|---|---:|---:|
| Native Antalya, single query / 4 threads | 1,896 [1,815, 2,196] | 2,105 [1,833, 2,452] |
| Native Antalya, 4 indexed batches / 1 thread each | 777 [711, 1,028] | 1,251 [1,097, 1,553] |
| Native Antalya, selective staging + 4 batches | 613 [508, 1,421] | 721 [626, 1,065] |
| Rust client, single query, FlushFirst | 2,337 [2,010, 2,733] | 2,765 [2,323, 3,169] |
| Rust client, selective staging + 4 batches, InBand reads | 647 [527, 1,350] | 742 [589, 2,552] |
| PostgreSQL FDW, EXPLAIN request | 2,778 [2,615, 3,057] | 2,734 [2,259, 2,835] |
| PostgreSQL FDW, complete binary COPY | 2,483 [2,265, 3,254] | 2,368 [2,246, 3,095] |
| Elasticsearch wildcard, 1 concurrent search | 5,271 [2,790, 10,840] | 3,980 [2,342, 7,822] |
| Elasticsearch wildcard, up to 4 concurrent searches | 1,331 [980, 2,223] | 1,010 [913, 1,623] |

Local shared-host results; activity and caches remain uncontrolled. Result-delivering
routes pass exact-pair checks; EXPLAIN uses row counts and identical-SQL COPY prechecks.
[Protocol, dense controls and local evidence](tests/benchmarks/README.md#current-requalification).

## Documentation

[Architecture](docs/architecture.md) · [Semantics](docs/semantics.md) ·
[SQL API](docs/sql-api.md) · [Operations](docs/operations.md) ·
[Testing](docs/testing.md) · [Contributing](AGENTS.md)

Apache-2.0 — [LICENSE](LICENSE) · [NOTICE](NOTICE).
