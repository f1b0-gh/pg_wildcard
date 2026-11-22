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

## Matcher cache configuration

Each PostgreSQL backend retains up to `pg_wildcard.matcher_cache_entries`
patterns (default 256) within `pg_wildcard.matcher_cache_kb` (default 32768 KiB).
Both settings are session-configurable; zero disables retention. Random eviction
avoids the clear-all cliff when a join cycles through more patterns than fit.
Changes apply on the next matching or diagnostics call.

```sql
SET pg_wildcard.matcher_cache_entries = 1024;
SET pg_wildcard.matcher_cache_kb = '64MB';
SELECT * FROM wc_matcher_cache_stats();
-- entries, charged_bytes, hits, misses, evictions (all bigint)
```

Counters are cumulative within the current backend. Diagnostics are volatile and
parallel-unsafe; parallel workers have separate caches. Charged memory excludes
compilation/search temporaries and caller-held references; it is not an RSS cap.
See [memory accounting](semantics.md) for library scratch-cache limits.

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
wc_ch_sweep_plan(source text, select_cols text[], col text, ids bigint[],
            regexes text[], views text[], max_ast_nodes integer = 45000,
            max_query_bytes integer = 200000, filter text = NULL,
            materialized boolean = false)
  → TABLE(query text, strategy text, source_scans bigint,
          estimated_ast_nodes bigint, exact_only_items bigint)
wc_ch_watchlist_ddl(name text, source text, hits text, watch text,
                    select_cols text[], col text,
                    views text = 'raw,lower,decoded,revdomain',
                    materialized boolean = false) → SETOF text
wc_ch_watchlist_insert(watch text, id bigint, regex text,
                       view text = 'raw') → text
```

These functions return statements or expressions; the caller executes them on
ClickHouse/Antalya. `materialized=true` uses conventional `<col>_<view>` columns.
`wc_ch_view_columns_ddl` adds derived columns, native n-gram text indexes and
index materialization statements. Its default list does **not** include
`winpath`; request it explicitly when needed.

Sweep arrays must have equal lengths. Each generated query returns the selected
columns plus `indicator_id` and `view`. Each indicator uses a constant regex and
a conservative candidate predicate before its exact `match` check. Batches count
the complete escaped SQL against their byte budget and a conservative parsed-AST
budget (45,000 nodes by default, below the engine default of 50,000);
a single indicator that cannot fit even after removing its optional candidate
returns an error. `wc_ch_sweep_plan` exposes configurable budgets, the strategy,
and total source-query count. Each query also includes these scan diagnostics
in a comment. Parsed-AST estimates do not replace the engine's expanded-AST or
execution limits; physical reads depend on the source plan and must be measured.
An optional `WHERE` gate uses up to three grams required by every indicator when
all indicators share a supported view/AND plan. It is omitted if unsupported,
alias-sensitive, or unable to fit the remaining byte/AST budgets; exact checks
and query count are preserved. Filter pushdown remains an engine-plan property.
There is no fixed indicator-count cap or automatic dynamic-regex switch. Short-circuit execution
depends on ClickHouse settings and expression evaluation; candidates do not
guarantee that every rejected row avoids regex work.

Watchlist DDL creates the indicator table and an insert-time materialized view
writing to the caller-provided hits table. It does not backfill existing source
rows or replay them when a watchlist changes. `wc_ch_watchlist_insert` returns an
INSERT with up to three conservative substring needles and their regex/view binding.
Only supported AND plans supply a needle; other plans use exact matching alone.
The view filters original indicator indices after each scalar substring check,
then rechecks the surviving indices exactly. It avoids nested arrays of needles. Default ranking penalizes common URL fragments and punctuation; the
library frequency-aware API can use the view-aware file counts emitted by
`wc-lake export-pg`. Counts rank already-proven required trigrams only; missing,
stale or incomplete counts cannot turn an optional gram into a requirement.
See [integration](../libwildcard/INTEGRATION.md) for the frequency APIs and metadata. Empty
or stale metadata bypasses the substring pre-check. Writers must use the helper
or compute equally sound metadata; bindings detect stale values, not deliberately
fabricated needles. The dynamic view still checks each indicator for each row;
benchmark ingestion with your data before enabling a large watchlist.

Hyperscan is not used because its Unicode and invalid-byte behavior can miss RE2
matches. See the [benchmark harness](../libwildcard/tests/benchmarks/README.md)
for comparisons with the previous implementations.

Existing watchlist tables need the new columns before executing new INSERT or
materialized-view SQL; `CREATE TABLE IF NOT EXISTS` does not migrate them:

```sql
ALTER TABLE watchlist ADD COLUMN IF NOT EXISTS needles Array(String) DEFAULT [];
ALTER TABLE watchlist ADD COLUMN IF NOT EXISTS needles_regex String DEFAULT '';
ALTER TABLE watchlist ADD COLUMN IF NOT EXISTS needles_view String DEFAULT '';
```

Use your actual ClickHouse table name, then recreate the existing materialized
view with the newly generated definition while coordinating source ingestion.
Existing rows receive empty metadata and retain exact results. Replacing a view
does not replay inserts made while the view was absent; pause ingestion or
arrange a replay if complete hit capture is required during migration.

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
wc_lake_sql_with_inventory(index_name text, lake_table text, select_list text,
             pattern text, known_table text, kind text = 'wildcard',
             view text = 'raw', extra_where text = NULL,
             materialized boolean = false) → text
```

All table columns are non-null. The file and gram tables reference
`wc_lake_index` with cascading deletion; their primary keys are
`(index_name, file_ord)` and `(index_name, gram)`. Load the CSV files produced by
`wc-lake export-pg`; file ordinals must be contiguous from zero.

`wc_lake_candidates` returns indexed candidate paths, including files with
unindexed values. A missing requested view or an `ALL` plan returns every
indexed path. It does not discover new files in the remote table.
`wc_lake_sql` retains the exact predicate and excludes only known impossible
files. Unknown paths remain searchable even when no indexed file is a candidate;
path guards exceeding 64 KiB fall back to an unrestricted file scan, with a
comment explaining the fallback in the returned SQL.

`wc_lake_sql_with_inventory` chooses the shorter bounded guard: inline excluded
paths, or a remote known-inventory subquery plus inline candidates. Load the
inventory artifacts from `wc-lake export-pg --known-table database.table` into
ClickHouse and pass that table name. Its `index_name` stores a digest of the exact
indexed path set, not the PostgreSQL index name. Missing or partial generations
only widen the scan; never insert foreign paths under an existing digest. Both
forms retain the exact row predicate. The FDW equivalent is `lake_known_table`.
Both functions' `select_list` and `extra_where` are trusted SQL fragments. Indexed paths must
identify immutable files;
see [lake coverage](lake.md#coverage-and-snapshot-rules).

## Foreign wrapper and planner support

The extension creates `wildcard_fdw`; its authentication, options, supported
types and limits are documented in the [FDW reference](fdw.md).

`wc_rewrite_support(internal)` is a planner hook attached to `wc_match`,
`wc_like` and `wc_regex`, not an application query function. It preserves
extension predicates on `wildcard_fdw` tables. For other relations, compatible
raw-view forms may become native LIKE or `re2match` calls. Regex rewrites use
only the matching function owned by the installed `re2` extension (pg_re2),
resolved in its extension schema independently of `search_path`. Non-raw views
keep their original calls: a similarly named foreign column does not prove
that it contains the requested transformation. PostgreSQL native operators
follow their own collation semantics, so this compatibility path is not a
replacement for the FDW's explicit checks.
