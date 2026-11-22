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

The standalone library also provides an opt-in byte-fragment index and bounded
rule-dispatch sweep. It selects candidate rule positions before exact regex
checks, preserving duplicate IDs and source rows. The index expression uses
hex-encoded byte windows so invalid UTF-8 remains searchable; an overflow token
keeps values beyond the extraction bound visible. Unsupported rule sets use an
explicit dense fallback with isolated source expressions. See
[consumer integration](../libwildcard/INTEGRATION.md#opt-in-byte-index-dispatch)
for limits and index-build requirements. This does not change default sweeps or
automatically create indexes. Evaluate write cost, storage and query latency
against the alternative indexed system on matching workloads.

## Puffin decoding and construction

Wildcard readers select `wildcard-ngram-v1` blobs; other Puffin blob types remain
separate. Decoding checks whether advertised counts can fit the remaining
encoded records and grows collections as records are validated. It does not
reserve an advertised maximum collection before reading its records.

`NgramIndex::decode` applies `DecodeLimits::default()`;
`decode_with_policy` accepts an explicit policy. The legacy container readers
apply the stricter policy below:

| Resource | Default n-gram decoder | `read_indexes` / `read_first_index` |
| --- | --- | --- |
| Encoded n-gram payload | 64 MiB | 32 MiB per decompressed blob |
| File entries | 65,536 | 128 per index |
| Distinct grams | 1,000,000 | 1,000,000 per index |
| Posting entries | 8,000,000 | 1,000,000 per index |
| Path bytes | 8 MiB | 64 KiB per index |
| Decoded index backing | 128 MiB | 128 MiB across returned indexes |

Both container readers reject files larger than 16 MiB. `read_indexes` returns
at most 16 matching indexes, charging their retained vector and string
capacities against the shared backing budget. `read_first_index` decodes only
the first matching blob after validating the footer; a malformed first match
is an error, not a reason to skip to a later index.

Generic Puffin readers cap encoded and decoded footer JSON at 1 MiB and each
decoded blob payload at 64 MiB; specialized readers may impose smaller limits.
Zstd frames have an 8 MiB window limit. The backing budget covers decoded index
collections, including spare capacity. Input files, footer metadata, a
decompressed payload, native decoder workspace, allocator overhead and temporary
reallocation overlap have separate lifetimes and are outside that budget.
These limits are not a total heap or process RSS guarantee.

`BoundedNgramIndexBuilder` admits at most 128 files, 64 KiB of paths, one million
distinct grams, one million posting entries and at most 8192 bytes per indexed
value. File/path admission failures return errors. Exhausting a gram or posting
budget marks the affected file as always requiring an exact recheck, as do
overlong values; it never silently drops that file from candidate results.
These construction limits bound counts, not total allocator bytes. The
compatibility `NgramIndexBuilder` remains a low-level builder for trusted
callers supplying their own aggregate admission policy. See
[lake indexing](lake.md) for file-read, Parquet and coverage requirements.

## PostgreSQL access

The scan-only [wildcard_fdw](fdw.md) generates exact search predicates and uses
the shared Antalya client with Arrow transport. It retains local checks of every
qual. Joins and aggregates remain PostgreSQL operations; pg_clickhouse can serve
general analytics separately.

The [SQL API](sql-api.md) also returns query text for external execution,
indicator sweeps and insert-time watchlists. These functions do not execute the
returned statements. See [build](build.md) for package prerequisites and
[operations](operations.md) for deployment guidance.
