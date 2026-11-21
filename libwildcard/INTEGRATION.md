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
