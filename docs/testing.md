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
| Performance correctness smoke and route profiling | `tests/benchmarks` | chDB for CI; isolated native engine, PostgreSQL and Elasticsearch for full measurements |
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
cargo build --locked --manifest-path libwildcard/Cargo.toml --features cold --bin wc-lake --example lake_scale_fixture
python3 libwildcard/tests/lake/iceberg_e2e.py libwildcard/target/debug/wc-lake \
  libwildcard/target/debug/examples/lake_scale_fixture
```

If `CARGO_TARGET_DIR` is set, use its `debug/wc-lake` path instead. The lake test
creates its own table under the current directory, compares pruned and full
results, checks append-after-index coverage, and corrupts a generated
non-candidate file to prove the engine skips it. The optional scale fixture
extends the local inventory to 10,000 paths (including empty historical files
absent from the current snapshot); the test loads the CLI-exported server
inventory and executes CLI-generated SQL. It checks stale generations, partial
loads, append preservation and physical skipping by fresh engines. It removes
its temporary directory on success; failures retain it for inspection. Run from
`.local/` with absolute executable/script paths to keep experiment artifacts
there.

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

The differential corpus also executes the opt-in watchlist candidate join as a
real materialized view. It compares complete multisets through rule updates,
deactivation, empty watchlists and stale needle bindings, including duplicate
source rows, multiple IDs sharing a pattern, qualified/empty projections,
internal-name collisions, materialized raw columns and values on both sides of
the extraction limit. Unicode and invalid-byte controls stay in the corpus.

For an already-owned `wildcard-benchmark-antalya-*` fixture, set
`ANTALYA_TEST_CONTAINER` to the exact container name and pass that name to
`tests/differential/run.py --antalya-container`. The helper checks the Docker
ownership label and creates a private database/user; it removes only those test
objects. Run qualification outside active timing measurements. Ownership
rejections are checked with:

```sh
python3 -m unittest discover -s libwildcard/tests/differential -p 'test_*.py' -v
```

## Consumer and transport tests

These Cargo workspaces use the published `antalya-rs` 0.1.1 crate; no sibling
checkout is required:

```sh
cargo test --locked -p wildcard-core -p wildcard-ch -p wildcard-puffin -p wildcard-lake
cargo test --locked --manifest-path tests/transport/Cargo.toml --all-features --all-targets
cargo test --locked --manifest-path tests/fdw/harness/Cargo.toml
```

The compatibility crates re-export the canonical library. The transport crate's
worker binaries support its regression tests; they are not publication targets.
See [transport checks](../tests/transport/README.md) and
[FDW checks](../tests/fdw/README.md) for scope and native-fixture setup.

## PostgreSQL

The qualification targets are PostgreSQL **18.6** and **19.0**, with pgrx
**0.19.4**. The default feature remains `pg18`. Initialize pgrx against that installation as described in
[build setup](build.md), then run as a non-root user:

```sh
cargo pgrx test pg18 -p pg_wildcard --no-default-features --features pg18 --cargo=--locked
cargo pgrx install -p pg_wildcard --no-default-features --features pg18 --pg-config "$PGRX_PG_CONFIG_PATH" --cargo=--locked
cargo build --locked --manifest-path libwildcard/Cargo.toml --features cold --bin wc-lake
PATH="$PGBIN:$PATH" python3 tests/fdw/run.py libwildcard/target/debug/wc-lake
```

To qualify PostgreSQL 19.0, use its `pg_config`/`PGBIN` and substitute `pg19`
for `pg18` in the commands above. Always pass `--no-default-features` when
selecting `pg19`; pgrx requires exactly one PostgreSQL major feature.

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
end-to-end test run without an Antalya checkout. The parity job also runs a
1% sweep/ingest correctness benchmark and uploads timings without performance
thresholds; see [measurement contracts and reproduction](../tests/benchmarks/README.md). The backend matcher-cache tests
also run directly with `rustc --test`, without PostgreSQL or a companion.
The separate consumer job resolves the published Antalya client and runs
compatibility crates, HTTP/Arrow transport and the PostgreSQL-independent FDW
harness without a companion checkout. See
[CI configuration](build.md#github-actions-configuration). The quality
job also runs locked Cargo metadata resolution for the fuzz workspace, so a
stale fuzz lockfile fails without requiring a nightly fuzz build.

Historical raw benchmark evidence is a separate checksummed release download.
Run `python3 tools/verify_benchmark_artifacts.py` to hydrate and verify it, or
dispatch the [archived evidence workflow](../.github/workflows/benchmark-evidence.yml).
The original three verifiers and ten-report reproduction run from disposable
copies under `.local/`. Normal tests do not download evidence. See
[artifact verification](../tests/benchmarks/ARTIFACTS.md) for offline use, retained
extractions and the automatic 256 KiB file / 2 MiB PR blob limits scoped to
`tests/benchmarks/results/`.

For branch protection, require these exact automatic check names:

- `Standalone library (default)`
- `Standalone library (all features)`
- `Format, policy, Clippy and package`
- `ClickHouse parity and Iceberg lake`
- `Companion transport and FDW`

Do not require the manual PostgreSQL job as an automatic pull-request check;
it is only created when its qualification workflow is dispatched.

The manual [PostgreSQL qualification workflow](../.github/workflows/postgres.yml)
runs the extension and FDW integration tests on PostgreSQL 14–19. Its PostgreSQL
18 and 19 lanes require 18.6 and 19.0 respectively, and fail if the package
source no longer provides them; 14–17 lanes use available minor releases as compatibility checks. These manual
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
