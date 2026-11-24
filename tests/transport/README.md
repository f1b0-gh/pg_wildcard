<!-- SPDX-License-Identifier: Apache-2.0 -->
# Transport regression tests

This independent, unpublished Cargo workspace exercises JSON and Arrow HTTP
transport, framing and completion checks, input limits, credential redaction,
connection reuse and worker error handling. It uses the published `antalya-rs`
0.1.1 crate described in [build setup](../../docs/build.md).

From the repository root:

```sh
cargo fmt --manifest-path tests/transport/Cargo.toml -- --check
cargo test --locked --manifest-path tests/transport/Cargo.toml --all-features --all-targets
cargo clippy --locked --manifest-path tests/transport/Cargo.toml --all-features --all-targets -- -D warnings
cargo deny --manifest-path tests/transport/Cargo.toml --all-features --config deny.toml check licenses
```

Tests create local HTTP fixtures and do not need a deployed service. The
`production` feature enables the companion Arrow client tests and the
`client-matrix-worker` target. The main executable and worker binaries remain
test support code, including the local simulated Elasticsearch error fixtures.
They do not establish live-engine performance or comparative benchmark results.

The separate [FDW harness](../fdw/harness/README.md) compiles the actual
PostgreSQL transport and value-decoder modules by path.

The `sweep_client` example exercises persistent clients against an explicitly
owned, immutable benchmark database. It accepts bounded JSON-line plans generated
by the canonical sweep planner and runs single, four-batch or selective staged
sweeps. Build it with `--release --features production --example sweep_client`;
use the [client experiment runner](../benchmarks/README.md#rust-client-parallel-sweeps)
to supply credentials and validate complete results.

This executor uses typed errors and forbids unsafe code, production `unwrap`,
`expect` and panic calls. Client clones share admission and retained-byte limits;
cooperative cancellation drains admitted tasks before dropping staging tables.
An unconfirmed cleanup makes the worker fatal and the parent removes its owned
fixture. Four requests share one Tokio worker thread: query concurrency does not
require four CPU-bound Rust workers. Server queries set `max_threads=4` for a
single query or `max_threads=1` per concurrent batch, inside a four-CPU fixture;
these settings do not cap every auxiliary server thread. These are benchmark
integration policies, not a production snapshot or multi-tenant execution service.
