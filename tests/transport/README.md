<!-- SPDX-License-Identifier: Apache-2.0 -->
# Transport regression tests

This independent, unpublished Cargo workspace exercises JSON and Arrow HTTP
transport, framing and completion checks, input limits, credential redaction,
connection reuse and worker error handling. It uses the published `antalya-rs`
0.1.1 crate described in [build setup](../../docs/build.md).

From the repository root:

```sh
cargo fmt --manifest-path tests/transport/Cargo.toml -- --check
cargo test --locked --manifest-path tests/transport/Cargo.toml --all-features
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
