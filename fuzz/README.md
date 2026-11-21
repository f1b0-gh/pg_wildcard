<!-- SPDX-License-Identifier: Apache-2.0 -->
# Sparse identity decoder fuzzing

Run from the repository root with `cargo-fuzz` and the specified nightly toolchain:

```sh
cargo +nightly-2026-10-29 fuzz run identity_codec fuzz/corpus/identity_codec -- \
  -max_total_time=600 -max_len=1048576
```

The target exercises full Puffin envelopes and raw v1/v2 identity payloads in a
valid envelope. Successfully decoded inputs must round-trip through v2 without
loss. The 1 MiB mutation limit complements the fixed budget and compatibility
tests; fuzzing is not an exhaustive safety proof. Generated corpora and crash
artifacts are ignored. Retain regression inputs in the maintained test fixtures.

This independent Cargo workspace uses development-only `libfuzzer-sys` 0.4.10,
licensed `(MIT OR Apache-2.0) AND NCSA`. Its narrowly scoped license exception is
in [deny.toml](deny.toml). It does not add dependencies to production builds.
