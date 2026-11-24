<!-- SPDX-License-Identifier: Apache-2.0 -->
# pg_wildcard

PostgreSQL extension for wildcard security-string searches, indicator handling,
ClickHouse/Antalya SQL generation, Arrow FDW scans and conservative lake pruning.
The canonical matcher and query generator are provided by `libwildcard 0.2.2`;
release 0.2.2 uses the qualified `antalya-rs 0.1.1` transport.

## Performance qualification

The repository's [current matrix](https://github.com/f1b0-gh/pg_wildcard/blob/main/README.md#measured-performance)
compares native Antalya, direct client, PostgreSQL and Elasticsearch sweeps,
including opt-in native batching and staging qualified on 2026-11-24. Those
routes do not add automatic parallel execution to the FDW. The separate
2026-11-23 FDW requalification
uses client 0.1.1 and the planned PostgreSQL 18.6 stack. It does not demonstrate
a consistent FDW optimization gain: results vary by workload, with paired ranges
crossing zero. Local measurements are not a general speed guarantee.

See the [benchmark guide](https://github.com/f1b0-gh/pg_wildcard/blob/main/tests/benchmarks/README.md#current-requalification)
for settings, exact-result checks and evidence availability. Earlier measurements
and flamegraphs remain historical evidence, separate from this candidate.

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

To upgrade an existing 0.2.0 or 0.2.1 installation, install the 0.2.2 shared library and
SQL files, reconnect so the backend loads the new library, then run:

```sql
ALTER EXTENSION pg_wildcard UPDATE TO '0.2.2';
```

SQL signatures are unchanged. The update records the new extension version.

The package also declares PostgreSQL 14–17 features; each server major requires its
own build and qualification. The FDW requires an Antalya endpoint and a bearer
user mapping. Compression is not exposed as a production FDW option.

See the repository's [build guide](https://github.com/f1b0-gh/pg_wildcard/blob/main/docs/build.md),
[SQL API](https://github.com/f1b0-gh/pg_wildcard/blob/main/docs/sql-api.md) and
[FDW guide](https://github.com/f1b0-gh/pg_wildcard/blob/main/docs/fdw.md).
License and third-party notices are included in this package.
