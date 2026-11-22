<!-- SPDX-License-Identifier: Apache-2.0 -->
# Testing

Run commands from the repository root unless stated otherwise. Use Rust 1.99.1
and the checked-in lockfiles. [Build setup](build.md) covers the compiler,
PostgreSQL and registry-client prerequisites.

| Suite | Location | Requirements |
| --- | --- | --- |
| Library unit, property and integration tests | `libwildcard/src`, `libwildcard/tests/*.rs` | Rust; no PostgreSQL or companion checkout |
| Rust/ClickHouse semantic parity | `libwildcard/tests/differential` | Rust, Python 3, chDB |
| Iceberg/Puffin end-to-end | `libwildcard/tests/lake` | Rust, Python 3, chDB, PyIceberg, PyArrow |
| Compatibility crates | `crates/wildcard-*` | Root workspace |
| HTTP/Arrow transport regressions | `tests/transport` | Rust |
| FDW transport and value decoders | `tests/fdw/harness` | Rust; no PostgreSQL server |
| Extension and FDW integration | `pg_wildcard`, `tests/fdw` | PostgreSQL, pgrx; chDB or owned Antalya fixture |
| Operations tooling | `tests/operations` | Python 3 standard library; native checks also need an owned Antalya fixture |

## Standalone library

```sh
cargo fmt --manifest-path libwildcard/Cargo.toml --all -- --check
cargo test --locked --manifest-path libwildcard/Cargo.toml
cargo test --locked --manifest-path libwildcard/Cargo.toml --all-features
cargo clippy --locked --manifest-path libwildcard/Cargo.toml --all-features --all-targets -- -D warnings
cargo package --locked --manifest-path libwildcard/Cargo.toml --all-features
cargo deny --manifest-path libwildcard/Cargo.toml --all-features --config libwildcard/deny.toml check licenses
```

The property tests require conservative pruning: extra candidates are allowed,
missing true matches are not. The `.puffin` files under
`libwildcard/tests/fixtures` are regression inputs and must remain in packages.

## ClickHouse parity and the lake

Use a Python virtual environment:

```sh
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install chdb "pyiceberg[sql-sqlite,pyarrow]"
python3 -m unittest discover -s libwildcard/tests/lake -v
```

Run the seeded semantic corpus in Bash so a failed generator also fails the
pipeline:

```bash
set -euo pipefail
for seed in 1 42 1234; do
  cargo run --locked -q --manifest-path libwildcard/Cargo.toml --features hot --example difftest_gen -- "$seed" |
    python3 libwildcard/tests/differential/run.py
done
cargo build --locked --manifest-path libwildcard/Cargo.toml --features cold --bin wc-lake
python3 libwildcard/tests/lake/iceberg_e2e.py libwildcard/target/debug/wc-lake
```

If `CARGO_TARGET_DIR` is set, use its `debug/wc-lake` path instead. The lake test
creates its own table under the current directory, compares pruned and full
results, checks append-after-index coverage, and corrupts a generated
non-candidate file to prove the engine skips it. It removes that directory on
success; failures retain it for inspection.

chDB validates the embedded engine version installed in the environment. For a
specific Antalya build, also run the native fixture. The standalone helper
requires Docker and its exact image already present locally, refuses image
downloads, checks the server version and removes only its own container:

```sh
cargo build --locked --manifest-path libwildcard/Cargo.toml --features hot --example difftest_gen
python3 libwildcard/tests/differential/qualify_antalya.py libwildcard/target/debug/examples/difftest_gen
```

The image digest and expected version are defined in
[`qualify_antalya.py`](../libwildcard/tests/differential/qualify_antalya.py).
That semantic fixture and the companion's framed-transport fixture test
different contracts; one does not replace the other.

## Consumer and transport tests

These Cargo workspaces use the published `antalya-rs` 0.1.0 crate; no sibling
checkout is required:

```sh
cargo test --locked -p wildcard-core -p wildcard-ch -p wildcard-puffin -p wildcard-lake
cargo test --locked --manifest-path tests/transport/Cargo.toml --all-features
cargo test --locked --manifest-path tests/fdw/harness/Cargo.toml
```

The compatibility crates re-export the canonical library. The transport crate's
worker binaries support its regression tests; they are not publication targets.
See [transport checks](../tests/transport/README.md) and
[FDW checks](../tests/fdw/README.md) for scope and native-fixture setup.

## PostgreSQL

The qualification target is PostgreSQL **18.6**, with pgrx **0.19.4** and the
`pg18` feature. Initialize pgrx against that installation as described in
[build setup](build.md), then run as a non-root user:

```sh
cargo pgrx test pg18 -p pg_wildcard --no-default-features --features pg18 --cargo=--locked
cargo pgrx install -p pg_wildcard --no-default-features --features pg18 --pg-config "$PGRX_PG_CONFIG_PATH" --cargo=--locked
cargo build --locked --manifest-path libwildcard/Cargo.toml --features cold --bin wc-lake
PATH="$PGBIN:$PATH" python3 tests/fdw/run.py libwildcard/target/debug/wc-lake
```

The chDB HTTP shim returns real Arrow values but simulates query-log completion.
Native authentication and terminal completion need the separate
[Antalya FDW fixture](../tests/fdw/README.md). Tests for other PostgreSQL majors
are explicit compatibility checks and do not qualify 18.6.

## Operations, publication and CI

```sh
python3 -m unittest discover -s tests/operations -v
python3 tools/check_publication.py
```

Native operations checks use the companion's owned framed fixture; see
[operations](operations.md). Fuzz targets have their own
[instructions](../fuzz/README.md).

The automatic [CI workflow](../.github/workflows/ci.yml) runs on pushes and pull
requests. Standalone default/all-feature tests, formatting, Clippy, package
verification, license checks, seeded ClickHouse parity and the Iceberg/Puffin
end-to-end test run without an Antalya checkout. The backend matcher-cache tests
also run directly with `rustc --test`, without PostgreSQL or a companion.
The separate consumer job resolves the published Antalya client and runs
compatibility crates, HTTP/Arrow transport and the PostgreSQL-independent FDW
harness without a companion checkout. See
[CI configuration](build.md#github-actions-configuration). The quality
job also runs locked Cargo metadata resolution for the fuzz workspace, so a
stale fuzz lockfile fails without requiring a nightly fuzz build.

For branch protection, require these exact automatic check names:

- `Standalone library (default)`
- `Standalone library (all features)`
- `Format, policy, Clippy and package`
- `ClickHouse parity and Iceberg lake`
- `Companion transport and FDW`

Do not require the manual PostgreSQL job as an automatic pull-request check;
it is only created when its qualification workflow is dispatched.

The manual [PostgreSQL qualification workflow](../.github/workflows/postgres.yml)
runs the extension and FDW integration tests on PostgreSQL 14–18. Its PostgreSQL
18 lane requires 18.6 and fails if the package source no longer provides it;
14–17 lanes use available minor releases as compatibility checks. These manual
runs are additional evidence, not part of the automatic CI result. Native Docker
fixtures and real DNS-stall tests remain separate qualification work.

Actions are pinned by commit, Rust is pinned to 1.99.1, and the Python interpreter
is 3.12. [Python test engines](../.github/requirements.txt) have exact direct
pins; CI logs the resolved transitive packages and ClickHouse engine version.
Those Python packages do not determine the Rust Arrow ABI. The
[dependency policy checker](../tools/check_dependency_policy.py) validates exact
Arrow/Parquet manifest constraints and every retained lockfile, including
transitive/test dependencies, against the qualified Antalya/ClickHouse client set:

```sh
python3 -m unittest discover -s tests/ci -v
python3 tools/check_dependency_policy.py
# Also validate an explicitly prepared companion checkout:
python3 tools/check_dependency_policy.py --companion ../antalya-rs
```

[Dependabot](../.github/dependabot.yml) groups Arrow, Parquet, ClickHouse and
Antalya. Automatic Arrow/Parquet major updates are excluded until an
explicit migration is qualified. A coordinated version update must also update
the policy checker and all affected lockfiles; grouping alone is not evidence
of compatibility. All consumer manifests resolve the published Antalya client
from crates.io. The standalone library is independently resolvable.
