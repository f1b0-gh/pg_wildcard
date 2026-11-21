<!-- SPDX-License-Identifier: Apache-2.0 -->
# Library tests

Run these commands from the `libwildcard` directory. This workspace is
independent of PostgreSQL and the companion transport client.

```sh
cargo test --locked
cargo test --locked --all-features
cargo clippy --locked --all-features --all-targets -- -D warnings
cargo package --locked --all-features
```

Unit and soundness property tests live beside the implementation in `src/`.
The Rust integration tests here cover the public API, lake coverage, immutable
identities, refresh, segment budgets and CLI behavior. Features enable the hot
SQL generator (`hot`), Puffin formats (`puffin`) and lake indexing (`cold`).

## Engine parity and lake integration

Create a Python 3 virtual environment and install the engine/test dependencies:

```sh
python3 -m venv .venv
. .venv/bin/activate
python3 -m pip install chdb "pyiceberg[sql-sqlite,pyarrow]"
python3 -m unittest discover -s tests/lake -v
```

Use Bash for the differential pipeline:

```bash
set -euo pipefail
for seed in 1 42 1234; do
  cargo run --locked -q --features hot --example difftest_gen -- "$seed" |
    python3 tests/differential/run.py
done
cargo build --locked --features cold --bin wc-lake
python3 tests/lake/iceberg_e2e.py target/debug/wc-lake
```

Use the corresponding paths under `CARGO_TARGET_DIR` if it is set. Differential
tests compare Rust results with generated ClickHouse SQL. The lake test compares
pruned and full scans, verifies newly appended files remain visible, and damages
one generated non-candidate file to check actual skipping. It creates its own
temporary table under the current directory and removes it on success.

The optional native runner needs Docker and the exact image from
[`differential/qualify_antalya.py`](differential/qualify_antalya.py) already
present locally. It refuses downloads, checks the expected engine version and
removes only its own container:

```sh
cargo build --locked --features hot --example difftest_gen
python3 tests/differential/qualify_antalya.py target/debug/examples/difftest_gen
```

## Fixtures and packaging

[`fixtures/legacy-v1.puffin`](fixtures/legacy-v1.puffin) and
[`fixtures/identity-v1.puffin`](fixtures/identity-v1.puffin) are committed binary
regression inputs used by `include_bytes!` in Rust tests. Keep them in the source
package along with the Rust tests and these Python runners. Test output, Python
caches and local virtual environments are not source fixtures.

chDB and PyIceberg dependencies are installed without a version lock; record
their versions when retaining qualification results. Passing chDB checks does
not qualify a different deployed engine build or its transport lifecycle.
