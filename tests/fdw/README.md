<!-- SPDX-License-Identifier: Apache-2.0 -->
# FDW qualification

Build with the pinned pgrx 0.19.4 / PostgreSQL 18.6 or 19.0 toolchain and the published
`antalya-rs` 0.1.1 crate. The FDW consumes Arrow using shared bearer authentication.
The native-engine commands below additionally need a sibling `antalya-rs` source
checkout for `scripts/fixture.py`; it is not used as a Cargo dependency.
Run from the pg_wildcard workspace:

```bash
export PGRX_PG_CONFIG_PATH=/path/to/postgresql-18.6/bin/pg_config
case "$("$PGRX_PG_CONFIG_PATH" --version)" in
  "PostgreSQL 18.6"|"PostgreSQL 18.6 ("*) ;;
  *) echo "PostgreSQL 18.6 is required" >&2; exit 1 ;;
esac
export PGBIN="$("$PGRX_PG_CONFIG_PATH" --bindir)"
cargo pgrx init --pg18 "$PGRX_PG_CONFIG_PATH"
cargo pgrx test pg18 -p pg_wildcard --no-default-features --features pg18 --cargo=--locked
cargo pgrx install -p pg_wildcard --no-default-features --features pg18 --pg-config "$PGRX_PG_CONFIG_PATH" --cargo=--locked
python3 ../antalya-rs/scripts/fixture.py python3 tests/fdw/antalya.py
```

For PostgreSQL 19.0, select its binaries and use `pg19` in the pgrx commands;
keep `--no-default-features`. Both targets run the same FDW tests below.

The disposable fixture supplies ANTALYA_TEST_URL and ANTALYA_TEST_TOKEN in the
child environment, validates actual JWT expiry/audience/signature failures,
and removes only its own pinned Antalya container. `antalya.py` starts its own
PostgreSQL cluster in a fresh private `/tmp/wc-fdw-antalya-*` directory, retains
its log, and drops its own Antalya database. `PGBIN` can select the PostgreSQL
binaries used for extension installation; otherwise `pg_config --bindir` from
the current PATH is used. Neither FDW harness recreates `/tmp/pg_wildcard`.
No existing deployment is restarted. Source
SQL/token-bearing mapping statements are never printed by the harness.

The broader semantic/lake test uses real chDB Arrow results and *simulated*
query-log terminal observations; this does not qualify real server completion:

```bash
cargo build --locked --manifest-path libwildcard/Cargo.toml --features cold --bin wc-lake
PATH="$PGBIN:$PATH" python3 tests/fdw/run.py libwildcard/target/debug/wc-lake
```

That shim accepts a fixed test bearer only, supports keepalive, and computes the
synthetic result-row count from its real Arrow output. Its timestamp source is
explicit DateTime64(6, UTC), because legacy chDB DateTime Arrow output is UInt32
and cannot safely satisfy a PostgreSQL timestamp contract. Real completion and
identity evidence come from the pinned Antalya run. Core `antalya-rs` fault tests
also cover truncated HTTP/IPC, split late exceptions, unconfirmed completion and
cancellation races. Unit tests under `pg_wildcard/src/fdw/` cover strict binary,
text, overflow and timestamp behavior plus bounded cache/concurrency and a
provisional batch followed by error/disconnect.

The shim also honors the requested Arrow IPC compression setting through a
fixed enum allow-list. This matters because chDB defaults to `lz4_frame`, whereas
the shared measured transport requests uncompressed IPC and refuses compressed
metadata before decoding. The test fixture must preserve that negotiation.

See [FDW behavior and limits](../../docs/fdw.md),
[build and CI setup](../../docs/build.md), and [testing](../../docs/testing.md).

`empty-u64.arrow` is a generated schema-only IPC fixture (no record batches),
created locally with `pyarrow.ipc.new_stream(path, pyarrow.schema([("n", pyarrow.uint64())]))`
and closing the writer. It contains no external data or copied code.

Projection/top-K, lazy dispatch, native candidate staging and late-error checks
are maintained in `optimization.py`; see [harness instructions](harness/README.md).
The native suite forces evaluation of columns that deliberately throw or sleep:
unused columns are correctly omitted by projection. Normal LIMIT/cursor close
and rescans now drain and confirm; cancellation remains an explicit abort.

The PostgreSQL 18.6/19.0 planner regressions run against an installed extension with no native
server. Use a C.UTF-8 database with ICU support:

```bash
psql -X -v ON_ERROR_STOP=1 -f tests/fdw/planner_topk.sql
psql -X -v ON_ERROR_STOP=1 -f tests/fdw/planner_collation.sql
psql -X -v ON_ERROR_STOP=1 -f tests/fdw/planner_identity.sql
psql -X -v ON_ERROR_STOP=1 -f tests/fdw/planner_predicates.sql
psql -X -v ON_ERROR_STOP=1 -f tests/fdw/lake_coverage.sql
```

The scripts roll back their objects. Predicate and coverage tests also check
PostgreSQL escape rules, session-independent temporal literals, function identity
and appended lake files. They cover qualified `wc_*` top-K, wrapper
aliases, same-named user functions, Unicode, OFFSET and conservative fallbacks;
ICU LIKE/equality and text ordering must not generate unsafe remote predicates,
candidate PREWHERE or lake pruning. `optimization.py` additionally checks actual
mixed-case ICU results and bounded `max_block_size` tuning. Its optional
`PG_OPT_PORT` selects a disposable cluster port (default 5432).
The native optimization suite checks executor timing ON/OFF, unused LIMIT0 scans
and rescans. `FDW row timing enabled=false` requires zero recorded conversion
time while retaining exact results, row counts and terminal proof. A unit test
uses a failing clock callback to ensure disabled timing never evaluates it.
