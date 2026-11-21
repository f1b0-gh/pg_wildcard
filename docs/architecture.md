<!-- SPDX-License-Identifier: Apache-2.0 -->
# Architecture

pg_wildcard searches security strings while preserving the original bytes as
evidence. Applications can keep indicators, watchlists, cases and search
definitions in PostgreSQL; the extension provides matching/indicator helpers,
SQL generation, lake index tables and foreign-table access. ClickHouse/Antalya
executes telemetry searches. An independent Rust library
provides the shared matching, planning, SQL generation and lake indexing logic.

```text
PostgreSQL control plane                  Rust consumers
  wc_* matching and indicators              libwildcard
  wc_ch_* SQL generation                       |
  wildcard_fdw / wc_lake_*                      |
             |                                 |
             +-------- ClickHouse/Antalya ------+
                         |              |
                  MergeTree tables   Iceberg tables
                  native text indexes   |
                                    Parquet files
                                    Puffin sidecars
                                      ^
                                    wc-lake
```

## Package boundary

`libwildcard/` is an independent Cargo workspace with its own manifest,
lockfile, toolchain and tests. It builds and packages without PostgreSQL or a
consumer checkout. Matching and planning are always available; optional features
add the following modules:

| Feature | Capability |
| --- | --- |
| `hot` | `antalya`: SQL generation, candidate predicates and storage settings |
| `puffin` | Puffin containers and candidate indexes |
| `cold` | Parquet index construction and the `wc-lake` binary; includes `hot` and `puffin` |

The default feature set is empty. The library generates SQL but does not own an
HTTP execution client. The outer workspace contains the pgrx extension and
`wildcard-core`, `wildcard-ch`, `wildcard-puffin` and `wildcard-lake` compatibility
crates. Those crates re-export the canonical library types, are not publication
targets, and do not maintain separate implementations. The legacy `wc-lake`
target uses the canonical library's CLI source.

## Query contract

A pattern, language and view produce an exact Rust matcher, a conservative
trigram plan, and an equivalent native SQL predicate. ClickHouse is the semantic
reference. Differential tests compare the implementations; see
[matching semantics](semantics.md) and [testing](testing.md).

Trigram plans are necessary conditions: they may retain extra candidates but
must never remove a true match. Exact predicates still execute after pruning.
Missing view coverage, unsupported analysis and exhausted pruning budgets widen
the search. File indexes additionally require correct coverage and immutable
file identities; [lake indexing](lake.md) explains the differences between the
CLI's unknown-file fallback and PostgreSQL's snapshot requirements.

Derived views are extra columns or expressions. They never replace the raw
evidence. Indicator normalization applies to the indicator being searched for.

## Hot storage

`wc_ch_view_columns_ddl` generates materialized view columns and native
`text(tokenizer = 'ngrams')` indexes, including index materialization statements.
`materialized=true` makes generated predicates target the conventional
`<column>_<view>` columns. The raw view uses the original column. A transformed
expression does not automatically benefit from an index on the raw column;
inspect native `EXPLAIN indexes=1` for the actual query.

`libwildcard::antalya::MergeTreeSearchLayout::Selective` provides an optional
CREATE TABLE settings fragment:

```sql
index_granularity = 512,
index_granularity_bytes = 10485760,
min_compress_block_size = 16384,
max_compress_block_size = 65536
```

It changes physical layout only. It does not execute DDL, rewrite existing parts
or select a layout automatically. Existing tables need an explicitly managed
migration where the engine treats these settings as immutable. Measure storage,
ingestion, broad scans and selective queries before adopting it; smaller granules
and blocks are not a universal improvement.

## PostgreSQL access

The scan-only [wildcard_fdw](fdw.md) generates exact search predicates and uses
the shared Antalya client with Arrow transport. It retains local checks of every
qual. Joins and aggregates remain PostgreSQL operations; pg_clickhouse can serve
general analytics separately.

The [SQL API](sql-api.md) also returns query text for external execution,
indicator sweeps and insert-time watchlists. These functions do not execute the
returned statements. See [build](build.md) for package prerequisites and
[operations](operations.md) for deployment guidance.
