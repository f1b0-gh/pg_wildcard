<!-- SPDX-License-Identifier: Apache-2.0 -->
# pg_wildcard

PostgreSQL extension for wildcard security-string searches, indicator handling,
ClickHouse/Antalya SQL generation, Arrow FDW scans and conservative lake pruning.
The canonical matcher and query generator are provided by `libwildcard 0.2.1`;
the extension uses the published `antalya-rs 0.1.0` transport.

## Measured performance

This comparison searches **one million documents with 1,000 wildcard rules**,
returning the same **10,000 exact row/indicator/view pairs** on every route.
The URL-shaped fields contain either shared literals (`*evil0000.example*`,
`*evil0001.example*`) or unrelated literals (`*e220a8397b1dcdaf*`, etc.).
These are substring searches, not exact hash lookups.

The first two numeric columns show historical matched sweep medians in
milliseconds (2026-11-22), before the 0.2.1 FDW changes. The final column shows
paired median latency reductions from the **0.2.1 FDW follow-up** (2026-11-23),
using a separate broad-recheck workload returning one million rows.

| Route | Shared structure (ms) | Unrelated literals (ms) | 0.2.1 broad-recheck follow-up: paired reduction |
|---|---:|---:|---|
| Native Antalya HTTP, refined byte index | 1,982.730 | 2,730.355 | — |
| Direct published `antalya-rs` 0.1.0 | 2,657.454 | 3,672.923 | Unchanged control |
| PostgreSQL backend request through FDW | 2,742.254 | 3,702.678 | **15.1%** (backend executor) |
| Full PostgreSQL binary COPY through FDW | 3,119.934 | 3,458.408 | **24.7%** (complete COPY) |
| Elasticsearch wildcard, 1 search at a time | 4,171.747 | 5,557.454 | — |
| Elasticsearch wildcard, up to 4 concurrent searches | 1,478.538 | 2,126.603 | — |

[Results, ranges and before/after flamegraphs](https://github.com/f1b0-gh/pg_wildcard/tree/main/tests/benchmarks/results/2026-11-23/fdw-overhead/).

**Elasticsearch's four-search mode is faster in both fixtures.** Its
[`_msearch` concurrency](https://www.elastic.co/docs/api/doc/elasticsearch/operation/operation-msearch)
counts concurrent rule searches. Antalya executes one combined query with
`max_threads=4`. This comparison does not establish multicore scaling.

### Methodology and limits

**These are local-machine measurements, not yet a reproducible benchmark.**
They have not been independently reproduced or validated in a controlled
benchmark environment.

- **Sweep setup:** Antalya 26.8.34.20001, Elasticsearch 9.5.6, PostgreSQL 18.6;
  four CPUs and 8 GiB per data engine, with a 4 GiB Elasticsearch heap.
  Direct/PG use `antalya-rs` 0.1.0 and uncompressed Arrow.
- **Sweep sampling:** one excluded warmup, five repetitions, rotating route order,
  one sweep at a time. Host activity and filesystem caches were uncontrolled.
- **Sweep timing:** backend requests use `EXPLAIN ANALYZE` with row timing off;
  COPY includes TCP delivery. These computed-pair queries have no local
  `wc_*` recheck. Subtracting route medians does not isolate FDW overhead.
- **Scope:** wildcard fields and opt-in byte indexing. Ingestion and index
  construction are measured separately; keyword and hash lookups are excluded.

See the [detailed results and methodology](https://github.com/f1b0-gh/pg_wildcard/blob/main/tests/benchmarks/README.md#measured-performance)
for SQL, exact-result checks, cache settings, sample ranges, ingestion costs and
scripts for rerunning the experiments.

## Build and install

This crate supplies extension source. It requires Rust 1.99.1, cargo-pgrx 0.19.4,
PostgreSQL server headers, a C compiler and Clang/libclang. PostgreSQL 18.6 is the
qualified target. From an extracted `pg_wildcard` crate directory:

```sh
export PGRX_PG_CONFIG_PATH=/path/to/postgresql-18.6/bin/pg_config
cargo pgrx init --pg18 "$PGRX_PG_CONFIG_PATH"
cargo pgrx install --no-default-features --features pg18 \
  --pg-config "$PGRX_PG_CONFIG_PATH" --cargo=--locked
```

The installation user must be allowed to write PostgreSQL's extension directories.
Then enable the extension in the intended database:

```sql
CREATE EXTENSION pg_wildcard;
SELECT wc_match('https://example.org/admin', '*/admin', 'raw');
```

To upgrade an existing 0.2.0 installation, install the 0.2.1 shared library and
SQL files, reconnect so the backend loads the new library, then run:

```sql
ALTER EXTENSION pg_wildcard UPDATE TO '0.2.1';
```

SQL signatures are unchanged. The update records the new extension version.

The package also declares PostgreSQL 14–17 features; each server major requires its
own build and qualification. The FDW requires an Antalya endpoint and a bearer
user mapping. Compression is not exposed as a production FDW option.

See the repository's [build guide](https://github.com/f1b0-gh/pg_wildcard/blob/main/docs/build.md),
[SQL API](https://github.com/f1b0-gh/pg_wildcard/blob/main/docs/sql-api.md) and
[FDW guide](https://github.com/f1b0-gh/pg_wildcard/blob/main/docs/fdw.md).
License and third-party notices are included in this package.
