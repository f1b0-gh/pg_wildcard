<!-- SPDX-License-Identifier: Apache-2.0 -->
# libwildcard

Matching, query generation and lake indexes for security strings. This Apache-2.0
Rust package is the canonical implementation used by pg_wildcard. It builds on
its own, without PostgreSQL, a network client or sibling source checkouts.

| Feature | Modules | Purpose |
|---|---|---|
| Default | `core` | Byte-oriented views, wildcard/LIKE/RE2 matching, gram plans and indicators |
| `hot` | `core`, `antalya` | ClickHouse/Antalya predicates, view/index DDL, sweeps and watchlists |
| `puffin` | `core`, `puffin` | Puffin codecs and file, row-group, identity and segment indexes |
| `cold` | All modules | Parquet indexing/refresh and the `wc-lake` CLI |

```rust
use libwildcard::core::{Matcher, View};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let matcher = Matcher::wildcard("*/wp-admin/*.php", View::Decoded)?;
assert!(matcher.is_match(b"GET /%77p-ADMIN/a.PHP"));
# Ok(())
# }
```

For parallel callers, retain one scratch cache per worker:

```rust
use libwildcard::core::{Matcher, View};
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let matcher = Matcher::regex("evil", View::Raw, false)?;
let mut cache = matcher.create_cache();
assert!(matcher.is_match_with(&mut cache, b"evil.example"));
# Ok(())
# }
```

Caller-owned scratch avoids the shared matcher mutex. Its retained memory is
bounded per cache, independently of `Matcher::cache_charge()`; bound worker and
cache counts as well. A cache can be reused with another matcher safely.

## Contracts

Matching follows ClickHouse semantics. Views are explicit derived values; raw
values remain evidence. Gram plans and indexes may over-select, but must never
exclude a true match. Always retain the exact predicate after candidate pruning.

Cold consumers must preserve snapshot, field, file and coverage bindings.
Unknown files and unindexed values remain candidates. Prefer
`antalya::lake_query_with_coverage` when a table can contain files absent from
an index; `wc-lake sql` uses that guard. `antalya::lake_query` with an explicit
file list requires the caller to verify complete current coverage.

Wildcard indexes occupy their own Puffin files, separate from deletion vectors
or other statistics. The execution engine applies Iceberg deletes. Indexing and
query planning do not execute SQL, manage a server or establish permissions.

The [single performance table](https://github.com/f1b0-gh/pg_wildcard/blob/main/README.md#measured-performance)
compares native Antalya, antalya-rs, PostgreSQL and Elasticsearch sweeps with
shared-host measurement limits and exact-result checks.

## Build

Rust 1.99.1 is pinned for this package. Its lockfile resolves Arrow/Parquet to
59.3.3. Run these commands from this directory:

```sh
cargo test --locked
cargo test --locked --all-features
cargo fmt --all -- --check
cargo clippy --locked --all-features --all-targets -- -D warnings
cargo package --locked --all-features
cargo build --locked --features cold --bin wc-lake
```

Use `libwildcard = "0.2.3"` from crates.io, or build its self-contained source
package. See [consumer integration](INTEGRATION.md) and [tests](tests/README.md).

## Optional hot-storage helpers

`antalya::candidate_predicate` derives a bounded necessary LIKE condition from
an existing gram plan. Apply it to the selected view and keep the exact predicate.
Unsupported conditions widen conservatively. Extra predicates can duplicate
engine work, so measure their effect before enabling them.

`antalya::MergeTreeSearchLayout::Selective.settings_sql()` provides settings for
new MergeTree tables: 512-row granules, adaptive marks and 16–64 KiB compression
blocks. It generates SQL; it does not alter an existing table or perform a
migration. Compare ingestion, storage, broad scans and selective queries before
choosing the profile; no universal speedup is promised.

```sh
cargo run --locked --features hot --example print_ddl -- --selective-layout
```

See [LICENSE](LICENSE) and [NOTICE](NOTICE) for redistribution terms.
