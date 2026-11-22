<!-- SPDX-License-Identifier: Apache-2.0 -->
# PostgreSQL-independent transport checks

This test-only Cargo workspace includes the production FDW transport and value
decoder by path. It adds no production dependency or second implementation.
It uses the published `antalya-rs` 0.1.0 crate in [build setup](../../../docs/build.md)
and needs no running PostgreSQL server. Run from the repository root:

```sh
cargo test --locked --manifest-path tests/fdw/harness/Cargo.toml
cargo clippy --locked --manifest-path tests/fdw/harness/Cargo.toml --all-targets -- -D warnings
```

For PostgreSQL 18.6 native planner and completion checks, install the extension in
an owned PostgreSQL container, prepare a sibling `antalya-rs` source checkout
for its fixture script, set PG_OPT_CONTAINER and PG_OPT_SOCKET, then run:

```sh
python3 ../antalya-rs/scripts/fixture.py python3 tests/fdw/optimization.py
# Separate, opt-in qualification of the newer framing-capable image:
PG_OPT_IN_BAND=1 python3 ../antalya-rs/scripts/fixture.py --framed python3 tests/fdw/optimization.py
```

The PostgreSQL container must be reachable on Docker's default bridge and expose
this checkout at the same absolute path, including the `.arrow` fixtures beside
`late_failure.py`. Its name must match the owned-container allow-list enforced
by [`optimization.py`](../optimization.py); use a name such as
`libwildcard-opt-pg-local1`. The
fixture creates isolated native tables, PostgreSQL foreign tables/server and
removes them; use a disposable PostgreSQL database. Full-source credentials stay
in environment/stdin, never command arguments. Set PG_OPT_RESULT to retain a
compact plan/counter summary. Its early-failure HTTP case is explicitly simulated;
the remaining cases use the real digest-pinned Antalya engine and terminal log.

The automatic candidate cases compare cold and warm decisions in the same
PostgreSQL backend, accept useful 512-row marks and reject nonselective 8192-row
marks, and check unknown-reader fallback. Plain EXPLAIN and LIMIT 0 must stay
offline. The transport harness also checks authentication, late proof failure,
cancellation, cache isolation and a single deadline across probes and data.

The default image rejects `in_band`; the explicit newer fixture checks framed
completion with zero data control requests. Body wait includes server work and
scheduling; it is not pure network time. HTTP bytes exclude headers/TLS.
