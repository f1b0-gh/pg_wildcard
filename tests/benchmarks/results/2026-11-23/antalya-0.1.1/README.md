<!-- SPDX-License-Identifier: Apache-2.0 -->
# Published Antalya 0.1.1 compression rerun — 2026-11-23

Complete raw evidence and the original hash manifests are preserved in the
[checksummed release archive](../../../ARTIFACTS.md). From the repository root,
run `python3 tools/verify_benchmark_artifacts.py` to verify this historical run.


Zstd reduces the Arrow hit-pair response from **262,920 to 101,840 bytes (61.3%)**
in both workloads, confirmed in the direct client and PostgreSQL EXPLAIN metrics.
Latency changes are mixed. These measurements do not establish a consistent
speed-up from compression or from upgrading the uncompressed client.

This reuses the exact one-million-document, 1,000-rule specifications and SQL
from the [November 22 comparison](../../2026-11-22/perf-plan/byte-dispatch/).
Each query returns 10,000 row/indicator/view pairs; only these pairs cross the
Arrow connection. Direct access and the full PostgreSQL route remain distinct.
Elasticsearch was not rerun, so these results do not replace its matched comparison.

## Measured latency

Milliseconds: median (minimum–maximum), six measured samples after one excluded
warmup per variant. Direct and PG are separate sequential cohorts: compare codecs
within a route, not the difference between route medians as an overhead estimate.

| Route, published client and Arrow codec | Shared structure | Unrelated literals |
|---|---:|---:|
| Direct 0.1.0, none | 1,999 (1,962–2,116) | 2,920 (2,005–3,108) |
| Direct 0.1.1, none | 2,829 (2,005–3,003) | 2,045 (2,006–3,002) |
| Direct 0.1.1, Zstd | 2,438 (1,872–2,966) | 2,461 (1,966–3,064) |
| PG backend 0.1.1, none | 2,317 (1,651–2,414) | 2,516 (2,392–2,646) |
| PG backend 0.1.1, Zstd | 2,389 (1,628–2,578) | 2,562 (2,539–2,596) |
| PG COPY 0.1.1, none | 2,379 (2,246–2,481) | 2,442 (2,321–2,559) |
| PG COPY 0.1.1, Zstd | 2,414 (1,672–2,575) | 2,498 (2,356–2,578) |

Direct timing covers fetch, Arrow decoding, consuming every field and confirmed
completion, excluding worker stdout serialization and Python oracle hashing.
PG backend timing is `EXPLAIN ANALYZE` Execution Time with row timing off. COPY
measures the persistent TCP psql request through complete binary file delivery;
all fields are decoded and checked after timing. These are generated pair queries,
with no local `wc_*` recheck. Disabled row-conversion instrumentation reports zero;
it is not a measurement of free conversion.

The current FDW pins 0.1.0. PG numbers here use a **benchmark-only patch**, not a
released FDW option: published 0.1.1 plus server `arrow_compression=none|zstd`,
validated and included in the client-cache identity. Both PG codecs use the same
release binary. See [patch](../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-23/antalya-0.1.1/benchmark-fdw.patch`) and [build identity](pg-build.json).

## Settings and limits

- Same saved corpus/rule hashes, byte-trigram index, one combined query, native
  Antalya 26.8.34.20001, four engine CPUs and an 8 GiB container limit; PostgreSQL 18.6.
- `max_threads=4`, `max_block_size=8192`, `flush_first`, 16 MiB output-batch limit,
  64 MiB retained-byte budget. Zstd reserves 32.5 MiB per pending read. Batch charges
  do not measure whole-process memory.
- Direct variants rotate over six rounds, balancing positions. PG alternates forward
  and reverse order. Sessions persist; one sweep runs at a time. No builds or tests
  run during timed cohorts. Other host activity and filesystem caches are uncontrolled.
- Exact full-COPY and every-rule canaries precede PG timing. Every direct/COPY sample
  is checked against the saved oracle; backend samples check actual row counts on the
  same SQL. All 98 samples/warmups have unique correlated `QueryFinish` evidence and
  server-confirmed codecs. Profiles show text-index posting reads and no condition-cache hits.
- Direct disables query/condition caches explicitly; PG inherits the fixture defaults.
  Full settings are retained in profiles, including direct's 100k versus FDW's 1M
  result-row cap; both return only 10k rows. This is not a test at either cap.
- PG correlation flushes logs after each sample, outside its clock; direct profile
  collection follows each workload's timed cohort. Terminal-confirmation phase and
  shared-host variation matter. Overlapping server/client clocks and separate medians
  must not be added or subtracted to attribute overhead.
- Both direct workers have identical source and transitive dependencies; their
  registry antalya-rs release/checksum differs. [Build provenance](direct-build.json)
  and locks are retained. PG's lock also rebinds tempfile's existing getrandom edge,
  as recorded in its build identity; this is shared by its two codec variants.

## Evidence and reproduction

[Direct summary](direct/summary.json), [PG summary](postgres/summary.json), raw
JSONL profiles, queries, plans, DDL, original specifications, client sources,
locks, test/build logs and fixture restoration are included. Detailed experiments
and discussion stay in `.local/`. No credentials or private worker logs are included.

Run the offline verifier from the repository root:

```sh
python3 tools/verify_benchmark_artifacts.py
```

To repeat direct measurements, copy `sources/` into a fresh
`.local/perf/<run-name>/` directory (exactly this depth). The scripts resolve the
repository root from that location and load the original archived workload files.
Use the recorded base commit and pinned Rust toolchain. Set `CARGO_TARGET_DIR` to
`$PWD/.local/review-pg/target`, build each copied manifest with `--release --locked`,
adding `--features v011` only for 0.1.1. Start the pinned owned Antalya fixture with
`tests/benchmarks/fixtures.py start --engine antalya --state
.local/perf/<run-name>/antalya.private.json`, then run the copied `run.py`.
The image must already be available; the fixture helper never pulls implicitly.

For PG, archive base commit `8ae6be1a40664e5bb242cc75407226e69ed5a6ba` into a private
source tree and apply the recorded patch there. Build/install its release pg18
extension into an **owned disposable PostgreSQL 18.6 fixture**, using the repository
[build guide](../../../../../docs/build.md). The container must bind this checkout
at `/workspace`, support the existing `PgWorker` psql protocol and reach the native
fixture's private network. Create `.local/perf/execution/` for the psql output
files, set `WILDCARD_BENCH_PG_CONTAINER` explicitly and run the copied `pg_run.py` after builds/tests finish. Stop only owned fixtures and restore
any previously installed extension/network state, as in [cleanup receipt](../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-23/antalya-0.1.1/cleanup.json`).

Changing completion policy, data shape, returning raw documents, or testing a
remote network requires a new labelled cohort; these numbers do not predict it.
