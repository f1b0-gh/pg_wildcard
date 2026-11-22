<!-- SPDX-License-Identifier: Apache-2.0 -->
# Consumer integration

Use this package from an explicit source path or its verified source archive:

```toml
[dependencies]
libwildcard = { path = "path/to/libwildcard", default-features = false, features = ["hot"] }
```

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
