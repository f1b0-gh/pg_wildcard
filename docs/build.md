<!-- SPDX-License-Identifier: Apache-2.0 -->
# Build and installation

## Requirements

| Component | Requirement |
|---|---|
| Rust | 1.99.1, pinned in both workspaces |
| Arrow / Parquet | 59.3.3 in the retained lockfiles |
| PostgreSQL extension | PostgreSQL 18.6, pgrx 0.19.4, C compiler, Clang/libclang and server headers |
| Remote FDW transport | A compatible `antalya-rs` companion checkout using Arrow 59.3.3 |
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
parent workspace or sibling checkout. These instructions do not imply a
crates.io release.

Use `hot` for SQL generation, `puffin` for index codecs and `cold` for all modules
and `wc-lake`. See [consumer integration](../libwildcard/INTEGRATION.md).

## PostgreSQL workspace

Obtain a reviewed revision of the separate Apache-2.0 `antalya-rs` client and
place the two checkouts beside each other:

```text
workspace/
  antalya-rs/
  pg_wildcard/
    libwildcard/
```

The workspace manifest uses `../antalya-rs`. Keep one canonical path to each
package in a Cargo dependency graph; symlink aliases can create duplicate Rust
types. The client must provide the APIs used by the FDW and resolve Arrow 59.3.3.
A matching crate version alone is not an API or engine compatibility guarantee.

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

The extension also declares PostgreSQL 14–17 features for compatibility checks.
Each server major requires a separate build and test run; the presence of a
feature or CI matrix entry is not proof of a successful deployment.

## GitHub Actions configuration

The standalone library and ClickHouse parity jobs do not need the companion
repository. The automatic consumer job and manual PostgreSQL workflow check out
[`f1b0-gh/antalya-rs`](https://github.com/f1b0-gh/antalya-rs) at reviewed commit
`6a0f8723df13ebdb305902ee8ff2f04706a26181`. The current dependency is a source
checkout; CI does not assume an Antalya crate has been published.

To qualify another reviewed source revision, set both repository variables:

- `ANTALYA_RS_REPOSITORY`: the real `owner/repository` name.
- `ANTALYA_RS_REVISION`: its full 40-character commit SHA.

For a private companion, configure a dedicated read credential named
`ANTALYA_RS_TOKEN` in **both Actions secrets and Dependabot secrets** on this
repository. Dependabot-triggered pull requests do not receive Actions secrets.
Grant that credential read access to the companion repository; the ordinary
`GITHUB_TOKEN` is scoped to this repository and does not grant access to another
private repository. Public companion checkouts use the default token with
read-only contents permission.

Credentials are not persisted in either checkout. Fork pull requests cannot
read private repository secrets; if the companion is inaccessible, the consumer
check fails instead of silently skipping its tests. Configure public read access
or arrange a trusted qualification run before relying on that check.

The source SHA and dependency-policy check identify the tested client; a matching
crate version alone does not establish API or engine compatibility. The token is
only for source checkout; remote query authentication is configured separately.
See [tests and CI](testing.md#operations-publication-and-ci) for automatic versus
manual checks and their prerequisites.

## Licensing and distribution

Project source is Apache-2.0. Keep the applicable LICENSE and NOTICE files with
source and binary distributions. The development-only fuzz runtime has an NCSA
exception. Review dependency changes with the relevant
`cargo deny` configuration; exceptions are not general additions to the allow-list.
