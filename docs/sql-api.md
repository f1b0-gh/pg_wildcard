<!-- SPDX-License-Identifier: Apache-2.0 -->
# PostgreSQL SQL API

Install the extension with `CREATE EXTENSION pg_wildcard`. The signatures below
use SQL types; arguments with `=` have defaults. Views are `raw`, `lower`,
`decoded`, `revdomain`, `winpath`. Pattern kinds are `wildcard`, `like`, `regex`,
`iregex`. See [semantics](semantics.md) for their exact meaning.

## Matching and inspection

```text
wildcard_version() → text
wc_match(value text, pattern text, view text = 'raw') → boolean
wc_like(value text, pattern text, view text = 'raw') → boolean
wc_regex(value text, pattern text, case_insensitive boolean = false,
         view text = 'raw') → boolean
wc_regex_bytes(value bytea, pattern text,
               case_insensitive boolean = false) → boolean
wc_view(value text, view text) → text
wc_host(value text) → text
wc_reverse_domain(value text) → text
wc_url_decode(value text) → bytea
wc_explain(pattern text, kind text = 'wildcard', view text = 'raw') → text
wc_re2_check(pattern text) → text[]
```

These functions are immutable and parallel-safe; the argument-taking functions
are strict. `wc_view` replaces invalid UTF-8 for display; `wc_url_decode`
preserves decoded bytes. `wc_re2_check` returns an empty array for a portable
pattern. `wc_explain` returns a conservative trigram plan, not an execution plan.

```sql
SELECT wc_match('GET /blog/%77p-ADMIN/setup.PHP',
                '*/wp-admin/*.php', 'decoded');
SELECT wc_like('https://login.example.com/x', 'com.example.%', 'revdomain');
SELECT wc_regex(E'a\nb', 'a.b');
```

## Indicators

```text
wc_refang(value text) → text
wc_indicator(raw text) → TABLE(itype text, value text, view text, regex text)
```

`wc_indicator` returns one canonical search specification. Types include domain,
URL, IPv4, IPv6, MD5, SHA-1, SHA-256, email and other. Refanging and
normalization apply to the indicator, never the stored telemetry.

```sql
SELECT * FROM wc_indicator('hxxps://example[.]com/path');
```

## ClickHouse SQL generation

```text
wc_ch_view_expr(col text, view text) → text
wc_ch_predicate(col text, pattern text, kind text = 'wildcard',
                view text = 'raw', materialized boolean = false) → text
wc_ch_view_columns_ddl(table text, col text,
                      views text = 'raw,lower,decoded,revdomain') → SETOF text
wc_ch_sweep(source text, select_cols text[], col text, ids bigint[],
            regexes text[], views text[], filter text = NULL,
            materialized boolean = false) → SETOF text
wc_ch_watchlist_ddl(name text, source text, hits text, watch text,
                    select_cols text[], col text,
                    views text = 'raw,lower,decoded,revdomain',
                    materialized boolean = false) → SETOF text
```

These functions return statements or expressions; the caller executes them on
ClickHouse/Antalya. `materialized=true` uses conventional `<col>_<view>` columns.
`wc_ch_view_columns_ddl` adds derived columns, native n-gram text indexes and
index materialization statements. Its default list does **not** include
`winpath`; request it explicitly when needed.

Sweep arrays must have equal lengths. Each generated query returns the selected
columns plus `indicator_id` and `view`; batching groups indicators by view.
Watchlist DDL creates the indicator table and an insert-time materialized view
writing to the caller-provided hits table. It does not backfill existing source
rows or replay them when a watchlist changes. Multi-pattern candidates retain
an exact `match` recheck.

Column/table identifiers and pattern literals pass through the SQL builders.
`filter` is a caller-owned SQL expression, not an escaped data value. Only pass
trusted SQL fragments to it.

## Lake tables and functions

```text
wc_lake_index(index_name text PRIMARY KEY, column_name text, views text,
              snapshot_id bigint, loaded_at timestamptz DEFAULT now())
wc_lake_file(index_name text, file_ord integer, path text,
             record_count bigint, has_unindexed boolean)
wc_lake_gram(index_name text, gram bigint, files integer[])

wc_lake_candidates(index_name text, pattern text, kind text = 'wildcard',
                    view text = 'raw') → SETOF text
wc_lake_sql(index_name text, lake_table text, select_list text, pattern text,
             kind text = 'wildcard', view text = 'raw',
             extra_where text = NULL, materialized boolean = false) → text
```

All table columns are non-null. The file and gram tables reference
`wc_lake_index` with cascading deletion; their primary keys are
`(index_name, file_ord)` and `(index_name, gram)`. Load the CSV files produced by
`wc-lake export-pg`; file ordinals must be contiguous from zero.

`wc_lake_candidates` returns indexed candidate paths, including files with
unindexed values. A missing requested view or an `ALL` plan returns every
indexed path. It does not discover new files in the remote table.
`wc_lake_sql` retains the exact predicate and returns NULL when a usable plan
has no indexed candidate files. Its `select_list` and `extra_where` are trusted
SQL fragments. The caller must ensure the index covers the queried snapshot;
see [lake coverage](lake.md#coverage-and-snapshot-rules).

## Foreign wrapper and planner support

The extension creates `wildcard_fdw`; its authentication, options, supported
types and limits are documented in the [FDW reference](fdw.md).

`wc_rewrite_support(internal)` is a planner hook attached to `wc_match`,
`wc_like` and `wc_regex`, not an application query function. It preserves
extension predicates on `wildcard_fdw` tables. For other relations, compatible
forms may become native LIKE or `re2match` calls when pg_re2 is installed;
derived-column rewrites require a foreign table with the corresponding text
column. PostgreSQL native operators follow their own collation semantics, so
this compatibility path is not a replacement for the FDW's explicit checks.
