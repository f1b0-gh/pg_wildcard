<!-- SPDX-License-Identifier: Apache-2.0 -->
# Build and installation

## Requirements

| Component | Requirement |
|---|---|
| Rust | 1.99.1, pinned in both workspaces |
| Arrow / Parquet | 59.3.3 in the retained lockfiles |
| PostgreSQL extension | PostgreSQL 18.6, pgrx 0.19.4, C compiler, Clang/libclang and server headers |
| Remote FDW transport | Published `antalya-rs` 0.1.0 using Arrow 59.3.3 |
| Python integration tests | Python 3.12 and the dependencies in the [testing guide](testing.md) |

The matching library has no PostgreSQL or network-client dependency. Compression
features build native dependencies, so an appropriate C build toolchain is needed
for all-feature builds. Docker is needed only for tests using disposable servers.

## Standalone library and CLI

From the repository root:

```sh
cargo test --locked --manifest-path libwildcard/Cargo.toml
cargo test --locked --manifest-path libwildcard/Cargo.toml --all-features
cargo build --locked --manifest-path libwildcard/Cargo.toml --features cold --bin wc-lake
cargo package --locked --manifest-path libwildcard/Cargo.toml --all-features
```

The default output directory is `libwildcard/target`. `CARGO_TARGET_DIR` overrides
it. The package is self-contained: it has its own manifest, lockfile, toolchain,
license, sources and tests. Building the extracted source package requires no
parent workspace or sibling checkout. The crates.io library package is `libwildcard 0.2.1`.

Use `hot` for SQL generation, `puffin` for index codecs and `cold` for all modules
and `wc-lake`. See [consumer integration](../libwildcard/INTEGRATION.md).

## Crates.io packages

`libwildcard 0.2.1` is the standalone Rust library; select `hot`, `puffin` or `cold`
as needed. `pg_wildcard 0.2.1` is the PostgreSQL extension source package and depends
directly on that library. Compatibility wrappers remain private re-exports.

The extension is installed with cargo-pgrx from its extracted source package,
not with `cargo install pg_wildcard`. It includes its control file, test fixtures,
license and notices. Run the PostgreSQL build commands below from the extracted
package directory, omitting `-p pg_wildcard`. PostgreSQL headers and the matching
`pg_config` are still required; crates.io does not distribute a prebuilt extension.

## PostgreSQL workspace

The workspace uses the Apache-2.0 [`antalya-rs` 0.1.0](https://crates.io/crates/antalya-rs/0.1.0)
release from crates.io, pinned as `=0.1.0` with TLS enabled. The transport and FDW
test workspaces use the same release. Cargo downloads the client; no sibling
checkout is needed. Commit all three consumer lockfiles when updating the
client, and qualify its API and Arrow compatibility with the transport tests.

Install the pinned pgrx CLI, then select your PostgreSQL 18.6 installation:

```sh
cargo install cargo-pgrx --version 0.19.4 --locked
export PGRX_PG_CONFIG_PATH=/path/to/postgresql-18.6/bin/pg_config
"$PGRX_PG_CONFIG_PATH" --version
export PGBIN="$("$PGRX_PG_CONFIG_PATH" --bindir)"
cargo pgrx init --pg18 "$PGRX_PG_CONFIG_PATH"
cargo pgrx install -p pg_wildcard --no-default-features --features pg18 \
  --pg-config "$PGRX_PG_CONFIG_PATH" --cargo=--locked
```

Check that the reported server version is 18.6. The installation user needs
permission to write PostgreSQL's extension directories. Run PostgreSQL tests
as a non-root user. In the intended database, enable the extension explicitly:

```sql
CREATE EXTENSION pg_wildcard;
```

For an existing 0.2.0 installation, install the new library and SQL files, then
reconnect and run `ALTER EXTENSION pg_wildcard UPDATE TO '0.2.1'`. The upgrade
script preserves the SQL API; existing sessions must reconnect to load the new
shared library.

The extension also declares PostgreSQL 14–17 features for compatibility checks.
Each server major requires a separate build and test run; the presence of a
feature or CI matrix entry is not proof of a successful deployment.
The qualification matrix also exercises FDW planner-private strings on each
major, including PostgreSQL 14's older `Value` node representation and the
PostgreSQL 17 extension-catalog cache identifier.

## GitHub Actions configuration

Consumer and PostgreSQL jobs resolve the client from crates.io using the
checked-in lockfiles. Standalone library jobs need no network-client dependency. The consumer job verifies compatibility crates, JSON/Arrow
transport, the PostgreSQL-independent FDW harness and dependency licenses.
The manual PostgreSQL workflow runs extension qualification across supported
server majors. No companion repository variables or checkout token are required.

The dependency-policy check pins Antalya 0.1.0, ClickHouse 0.15.3 and Arrow/Parquet
59.3.3. Registry checksums identify the release; transport tests establish API
compatibility. Remote query authentication is configured separately.
See [tests and CI](testing.md#operations-publication-and-ci) for automatic versus
manual checks and their prerequisites.

Optional native-engine tests still use `scripts/fixture.py` from a separately
prepared [`antalya-rs` source checkout](https://github.com/f1b0-gh/antalya-rs).
That checkout supplies test tooling only; Cargo continues using the registry
release. See [FDW qualification](../tests/fdw/README.md).

## Licensing and distribution

Project source is Apache-2.0. Keep the applicable LICENSE and NOTICE files with
source and binary distributions. The development-only fuzz runtime has an NCSA
exception. Review dependency changes with the relevant
`cargo deny` configuration; exceptions are not general additions to the allow-list.
