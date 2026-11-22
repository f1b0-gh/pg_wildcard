<!-- SPDX-License-Identifier: Apache-2.0 -->
# Performance measurements

Keep these routes separate when reading the results:

| Route | Measured work | Entry point |
|---|---|---|
| Native engine SQL over HTTP | Generated sweep predicates, source scans and ingest materialized views | `libwildcard/tests/benchmarks/sweeps.py --state …` |
| Direct `antalya-rs` | Published Rust client, HTTP transport, Arrow decoding and batch consumption | `tests/transport/examples/fdw_profile.rs` |
| PostgreSQL backend | PostgreSQL executor, `antalya-rs`, Arrow-to-datum conversion and local condition rechecks; EXPLAIN consumes results | `fdw_profile.py` |
| Full PostgreSQL delivery | Backend work plus binary COPY serialization, TCP delivery and complete result consumption through psql | `fdw_wire.py` |
| Elasticsearch | HTTP wildcard queries over separately indexed `wildcard` and `keyword` fields | `elasticsearch.py` |

The first route does not exercise `antalya-rs`. Direct-client and PostgreSQL
measurements use identical remote projections and options per paired workload.
The Elastic comparison uses the same ASCII corpus and verifies the same sorted
row/indicator/view pairs as batch sweeps. Its `_msearch` result contract and
indexing cost are reported separately from boolean-union searches and ingest
materialized views. Elastic JSON cannot represent arbitrary invalid UTF-8;
semantic probes are not a general RE2 equivalence claim.

See [sweep reproduction](../../libwildcard/tests/benchmarks/README.md),
[FDW profiling](FDW.md), and [committed measurements](results/2026-11-22/).
Detailed experiments and private engine state belong in `.local/`; only the
requested sanitized raw measurements, settings and their summaries are committed.

## Owned fixtures

`fixtures.py` creates isolated Docker networks and containers from locally cached,
digest-pinned images. It never uses an existing service or pulls an image. Start
an engine with `python3 tests/benchmarks/fixtures.py start --engine antalya
--state .local/bench/antalya.json` (or `--engine elasticsearch`). The private state
contains credentials and must not be copied into results. Stop it with the same
command using `stop` and omitting `--engine`. Disconnect any profiling PostgreSQL
container from that owned network before stopping the fixture.

Run timed suites sequentially. Record resource limits, engine versions, cache
settings, exact result checks and failed/censored attempts. Engine and client
clocks overlap; adding them does not produce a stage breakdown. A retained batch
budget is not a process RSS cap. Shared development-host timings are descriptive,
not a universal speed guarantee.

CI runs 10,000 rows (1% of the full corpus) against all generated sweep and ingest
strategies, checks exact result pairs, adds an untimed positive canary for every
indicator across all sweep batches, and uploads timings without enforcing
performance thresholds.
