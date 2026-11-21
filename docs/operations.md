<!-- SPDX-License-Identifier: Apache-2.0 -->
# Deployment diagnostics and selective-layout migrations

`tools/wildcard_ops.py` is a Python 3 standard-library tool. `diagnose` sends
bounded, read-only queries; `plan` sends the same queries and prints a review
document containing SQL. Neither command executes a migration. Exit status 2
means refusal or failed capability checks; status 0 does **not** approve a
production deployment. Protect the report: it contains the selected table's
schema metadata, although no authentication fields, raw DDL, data values,
policy expressions or server error text are printed.

## Reproducible preparation

Use the repository's pinned Rust toolchain and lockfiles. Build the maintained
profile emitter from the independent library workspace:

```sh
cargo build --locked --manifest-path libwildcard/Cargo.toml --features hot --example print_ddl
cargo test --locked --manifest-path libwildcard/Cargo.toml
cargo package --locked --manifest-path libwildcard/Cargo.toml --all-features
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests/operations -v
```

The plan reads settings from that executable's `--selective-layout` output;
there is no second implementation of the profile in Python. If the emitter's
format changes, planning refuses. Treat the executable as trusted deployment
code. Its path is normally `libwildcard/target/debug/examples/print_ddl`, or
under the explicitly selected `CARGO_TARGET_DIR`.

This profile's engine qualification is Antalya
`26.8.34.20001.altinityantalya`, image
`altinityinfra/clickhouse-server@sha256:0989fbcd21552463cff9ef8d7a44bd2213a2c8e1879816f856a744a14b28e77a`.
Check the deployed image digest independently; `version()` cannot attest it.
An arbitrary newer engine is not accepted by this tool. The framing-setting
check proves availability of a setting, not successful terminal proof delivery.
Re-run the client qualification for the actual transport before deployment.

For consumers installing PostgreSQL, separately build/install the extension
with `cargo pgrx install -p pg_wildcard --pg-config <pg18-pg_config>` and test
with `cargo pgrx test pg18 -p pg_wildcard`. The qualification target is 18.6.
Use your deployment's protected libpq service configuration and inspect
`SHOW server_version` and `SELECT extname, extversion FROM pg_extension`.
Do not automatically run `ALTER EXTENSION UPDATE`: package availability and
the database's installed version are separate facts. This HTTP diagnostic
reports PostgreSQL extension state as `not_observed`.

## Connection and diagnostic

Create an owner-only regular JSON file through your secret manager or editor
(mode 0600; do not put credentials in a shell command). The accepted fields
are `endpoint`, and either `user` plus `password`, or `bearer`. HTTPS is required;
the endpoint must have no credentials, query string, fragment or non-root path.
Only an explicitly selected local test endpoint may use HTTP, with
`allow_loopback_http: true`. Redirects and environment proxies are disabled.
Use a principal restricted to the intended table and required system metadata;
the tool does not enumerate other teams' tables. Missing metadata permission
causes refusal, never a misleading successful report.

```sh
python3 tools/wildcard_ops.py diagnose --config /secure/connection.json \
  --database telemetry --table events
python3 tools/wildcard_ops.py plan --config /secure/connection.json \
  --database telemetry --table events --candidate events_selective \
  --profile-emitter libwildcard/target/debug/examples/print_ddl
```

Diagnostics show the observed engine version, required functions, selected
query settings, columns, text-index definitions, pending mutations, explicit
layout settings and a schema fingerprint. Missing explicit settings are
`null`, requiring review of server defaults. Existing parts can have another
layout. Defined indexes do not prove materialization, coverage or use in a
query: those fields remain `unknown`/`not_observed`. Missing text indexes are
reported as a warning; exact search can still work. The tool does not infer
Iceberg freshness from age, inspect another application's refresh state, or
claim a queried snapshot, pruning decision or query identifier it did not
observe. Obtain those from the executing consumer's authorized diagnostics.

## Review and execute a migration

The generated document is intentionally marked `executable: false`. Its SQL
is a concrete proposal to review and run individually through your controlled
deployment process. Planning supports plain `MergeTree` in an `Atomic`
database. Replicated, replacing, aggregating, distributed, external and other
engines are refused. Candidate-name collisions, pending mutations, unsafe
identifiers and missing capabilities are refused. There is no automatic
online-write reconciliation, policy copier or cutover authorization.

1. Verify the image digest and capture protected source DDL, UUID, settings,
   dependency inventory, role grants and row policies. Inventory views,
   materialized-view targets, dictionaries, caches and readers using UUIDs.
   Review TTL behavior, retention and sufficient disk space. A clone can keep
   source TTL and background actions; freeze or otherwise account for them
   under a reviewed procedure. Schedule a maintenance window for the entire
   copy and validation. This procedure does not support concurrent writes.
2. Review the proposed `create_sql`. On the pinned engine, `CREATE TABLE dst
   AS src SETTINGS ...` clones definitions and overrides the canonical layout
   settings at creation. It preserves the engine's schema definitions without
   interpolating their expressions into this tool's SQL. The new UUID is
   intentional. Do not use `ALTER index_granularity`, which the engine refuses.
   Run creation once with an unused name; no `IF NOT EXISTS` hides a collision.
3. Restrict access to the candidate **before copying data**. Review wildcard
   grants, inherited roles, table-name and UUID bindings, and policies on both
   names. Schema cloning does not clone authorization. Install reviewed policy
   equivalents and test positive and negative access. Do not publish raw grants
   or policy expressions in the general report.
4. Freeze and drain **all** writers, queues, asynchronous inserts, mutations and
   relevant background retention actions. Prevent restarts from re-enabling
   writers. Recheck source UUID/schema fingerprint and empty candidate. The
   copy identity must see the whole source; row-policy-filtered copies can
   appear equal while omitting inaccessible rows. Use a specifically reviewed
   migration identity; test end-user policies separately.
5. Run `copy_once_sql`. The explicit insertion list excludes MATERIALIZED and
   ALIAS columns, which the clone computes. Do not replay INSERT after a lost
   connection or partial failure. Establish terminal completion and inspect
   the candidate; rebuild a fresh empty candidate under the same review if
   necessary. The source remains authoritative throughout preparation.
6. Require zero from `verify_multiset_sql` and every `verify_metadata_sql`
   query. `EXCEPT ALL` compares both directions and preserves duplicates;
   all columns, including derived columns, are compared. Unsupported types or
   resource-limit errors fail the verification; never replace it with a
   hash/count-only proof. Full comparison can be expensive. Privately compare
   complete source/candidate DDL, constraints, projections and non-layout
   settings as well; metadata queries are not a full DDL proof. Inspect actual
   new-part layout and materialized indexes, run the canonical exact predicate
   corpus, inspect EXPLAIN/index evidence and compare results and denials under
   every affected role. Save protected evidence with table UUIDs.
7. Obtain deployment authorization only after all gates pass. With writers
   still frozen, execute `cutover_sql` (`EXCHANGE TABLES`). Check UUID-to-name
   mapping, dependencies, query results and per-role permissions again. Only
   then reopen writes. An exchange is atomic within the supported database;
   it does not make all dependent applications or authorization state atomic.

## Failure and rollback

Before cutover, any failure leaves the source authoritative. Keep writers
paused while determining whether an interrupted operation completed. Never
blindly retry a copy or exchange. After cutover but **before any new writes**,
verify the UUID mapping and use `rollback_before_writes_sql` to exchange back;
recheck results and permissions before reopening writes.

After writes resume, reverse exchange alone can lose new writes. The tool
therefore emits `rollback_after_writes_sql: null`. Freeze again and use a
separately reviewed reconciliation/replay strategy covering all new writes,
updates and deletes, then verify exact equality and permissions before a
reverse exchange. Retain the previous table through the rollback window;
there is no automatically generated DROP or cleanup action.

## Local validation and limits

`tests/operations/native.py` runs only inside the disposable digest-pinned
Antalya fixture and creates a randomly named owned database. It tests the
read-only diagnostic, canonical layout creation, native schema clone, exact
copy including duplicates, corruption detection, candidate rebuilding,
exchange, rollback and a restricted role's row policy and column denial.
It accepts a private `--work` directory and trusted `--profile-emitter`.
Invoke it as the callback to `../antalya-rs/scripts/fixture.py --framed`.
The helper owns and removes only its container; `--hold` is for coordinated
local testing and ends when `<work>/release` is created.

These are local fixture checks, not qualification of generic grants, complex
dependencies, concurrent production writers, disaster recovery or arbitrary
table types. The tool adds no SQL surface or library dependency. Engine cloning
and exchange syntax are described in the official
[CREATE TABLE](https://clickhouse.com/docs/sql-reference/statements/create/table)
and [EXCHANGE](https://clickhouse.com/docs/sql-reference/statements/exchange)
documentation; the pinned native tests determine which behavior is qualified
here.
