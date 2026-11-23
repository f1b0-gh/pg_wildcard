<!-- SPDX-License-Identifier: Apache-2.0 -->
# Consumer integration

Use the standalone package from crates.io:

```toml
[dependencies]
libwildcard = { version = "0.2.0", default-features = false, features = ["hot"] }
```

For local development, replace the version with `path = "path/to/libwildcard"`.

Select `hot` for SQL generation, `puffin` for codecs or `cold` for Parquet indexing
and the CLI. Default features provide only matching, gram planning and indicator
handling. Rust 1.99.1 is pinned; Arrow/Parquet resolve to 59.3.3 in the lockfile.

| Compatibility import | Canonical import |
|---|---|
| `wildcard_core::` | `libwildcard::core::` |
| `wildcard_ch::` | `libwildcard::antalya::` |
| `wildcard_puffin::` | `libwildcard::puffin::` |
| `wildcard_lake::` | `libwildcard::lake::` |

The compatibility crates are re-exports, not independent implementations or
publication targets. Use one canonical package path per Cargo graph to preserve
public type identity. `puffin::codec` is the format module; `puffin::puffin` remains
an alias. Binary formats retain explicit versions and compatibility fixtures.

SQL helpers return text for the consumer to execute. They do not create a client,
runtime or authorization boundary. Use the execution client's limits, cancellation
and completion checks. Keep identifier/literal quoting, exact predicates and
current index bindings when composing generated SQL.

For changing lake inventories, prefer `antalya::lake_query_with_coverage` over an
explicit candidate allowlist. Unknown files must remain searchable. The caller
must establish immutable file identity, trusted index bindings and current
snapshot coverage; a sidecar alone does not establish any of these.

The PostgreSQL adapter lives in the parent pg_wildcard repository. It is not a
dependency of this package. Validate the adapter and actual engine separately
when integrating a library change.

## Sweep row gates

Sweeps retain constant exact checks and batch by escaped SQL bytes and estimated
parsed AST size. When every indicator uses the same view and bounded supported
AND plans, the planner can add up to three shared required trigrams as an
ordinary `WHERE` condition. This can reject rows before constructing indicator
arrays when the engine pushes the filter down. It does not change query count.
If a plan is unsupported, no shared gram exists, the remaining budget is too
small, or the value expression is trusted custom SQL or an unqualified generated
alias (`view`, `indicator_id`, `wc_hit`), the optional gate is omitted. Existing
filters stay parenthesized and every per-indicator exact check remains.

## Opt-in byte-index dispatch

`antalya::sweep_byte_dispatch` generates a single source query for a bounded set
of raw-view rules. It groups rule positions by required byte fragments, selects
candidate positions with byte substring searches, and applies exact `match()` to
those positions. Duplicate IDs and source rows retain their multiplicity. This
is independent of the regex engine and does not change the default sweep API.
The opt-in result uses `ByteDispatchSweepPlan` and `ByteDispatchStrategy`; the
original `SweepPlan` and exhaustive `SweepStrategy` remain unchanged.

Selected source values retain their order and multiplicity, including repeated
columns. Generated hit columns always end the result as `indicator_id` and `view`.
If selected source names collide with those labels, their result names are
qualified by a generated source relation. Distinct qualified source identifiers
sharing a final component retain their full identifier as the source column
label. Inspect response metadata for those source names; do not treat a source
`indicator_id` or `view` as the generated match. These projections use an extra
scope boundary, charged to the SQL/AST budget.

The current limits are 2,000 rules, 255 groups, 64 rules per group, and at most
256 unassigned rules or 25% of the input. Other views, insufficient coverage and
SQL/AST budget exhaustion return the existing constant-candidate plan with an
explicit fallback reason. A rule with no required fragment is still checked
exactly and disables the optional source index gate.

The source gate requires up to three proven fragments for each rule, with OR
between rules. This avoids admitting a row merely because it shares one common
fragment, such as digits in a benign URL. Long values bypass this gate. If the
conjunctions exceed the SQL or parsed-AST budget, the planner widens to one
fragment per rule before considering dense fallback. `gate_strategy` reports
`RequiredFragments`, `SingleFragmentBudget` or `Disabled`; `index_gate` only says
whether a source gate was emitted, not whether the engine used an index.

`byte_gram_index_ddl(table, index, column)` emits an explicit fragment-array text
index for the matching raw column. Existing parts require `MATERIALIZE INDEX`.
The expression extracts six-character windows from hex-encoded bytes, preserving
byte fragments even for invalid UTF-8. Windows crossing a half-byte boundary
only add candidates; exact regex checks remove them. This avoids copying the
source string per trigram inside an array lambda.
Index extraction stops at 8,192 bytes; longer values carry an overflow token and
remain candidates even when a match lies beyond that prefix. The gate has scalar
semantics with or without the index. Check physical index use and engine-version
compatibility separately. The source value is preserved.

This is an opt-in layout for admitted wildcard fields. Measure initial indexing,
sustained ingestion, storage and query latency against the alternative indexed
system on the same corpus and rules. An unindexed ClickHouse baseline measures
added overhead, not competitiveness with Elasticsearch's wildcard index.
Extraction bounds do not bound total query memory or promise a speedup. Optional
`GramFileCounts` only rank already-proven required fragments.

## Watchlist needle selection

`antalya::watchlist_needles(pattern, view)` selects up to three necessary ASCII
interior trigrams from a bounded pure-AND regex plan. Fixed ranking penalizes
punctuation and common URL grams and favors digits and rare letters. Unsupported
plans produce no needles. `watchlist_insert_sql` stores the needles with their
source regex and view; use this helper or leave derived metadata empty.
`watchlist_mv` filters the original indicator indices after each of the three
scalar substring checks, then applies the exact regex to surviving indices.
Empty slots and stale regex/view bindings safely widen matching. These stages
avoid a nested array of needles and do not depend on AND short-circuit evaluation.
They preserve index positions into the bound ID, regex and needle arrays.

`antalya::watchlist_plan` adds explicit `WatchlistStrategy` selection and returns
the materialized-view SQL, selected strategy, fallback reason and extraction
bounds. `StagedNeedles` is the default and keeps the existing SQL. The opt-in
`RuleAnchorJoin` currently requires exactly `[View::Raw]`; other view sets use
staged needles with `RequiresSingleRawView`. Empty or duplicate view lists are
errors. The existing `watchlist_mv` entry point is unchanged.
The returned strategy describes generated DDL; `IF NOT EXISTS` does not change
the definition of a materialized view that already exists.

The join routes each source row's distinct byte trigrams to one required anchor
per active rule, then checks its bound needles and exact regex. Values over 256
bytes and rules without a usable anchor use fallback join keys. Extraction is
bounded to 255 source keys per row independently of short-circuit settings.
That cap does **not** bound the number of joined rules or total query memory:
unselective watchlists still require engine memory/time limits and measurement.
Only the first three needles participate, with the same regex/view bindings as
the staged strategy. Source duplicates and different IDs sharing a pattern are
preserved. Rule updates affect subsequent inserts, not historical hits.

Qualified projections and materialized raw-column mappings remain supported.
Internal names avoid referenced identifiers; exhaustion of the bounded alias
namespace returns staged SQL with `InternalAliasConflict`. This indexes rules
within each inserted block, not stored documents; it makes no physical file or
granule-skipping claim. Callers still enforce wildcard-field eligibility.

For corpus-aware ranking, pass `Some(&counts)` to
`watchlist_needles_with_frequencies` or `watchlist_insert_sql_with_frequencies`.
`GramFileCounts` is a `BTreeMap<core::grams::GramKey, u64>` of observed distinct-file
counts. Keys include the view. Lower known counts rank first, unknown counts rank
after known counts, and fixed ranking breaks ties. Missing entries never imply
zero. Statistics affect selection only: stale counts can hurt performance but
cannot make an optional gram required or suppress an exact match.

`wc-lake export-pg` also writes `gram-frequencies.csv` (header
`gram_key,file_count`) and `gram-frequencies.json`. The latter identifies the
format, complete path inventory coverage ID, column, snapshot, views, counts and
source Puffin properties. Counts are posting lengths, so repeated occurrences in
a file count once; values skipped by indexing limits are omitted. The metadata
reports files with unindexed values and explicitly marks missing counts unknown.
Applications should validate this provenance against the intended corpus before
loading the CSV into `GramFileCounts`. These statistics never authorize file
pruning. Library callers can emit the same files with
`lake::export_gram_frequencies` without changing the Puffin format.
