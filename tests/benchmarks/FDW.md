<!-- SPDX-License-Identifier: Apache-2.0 -->
# Native paired FDW profiling

Build the PostgreSQL extension with `cargo pgrx install --release` and the direct
published client with:

```sh
cargo build --locked --release --manifest-path tests/transport/Cargo.toml --features production --example fdw_profile
```

`CARGO_TARGET_DIR` changes the resulting direct-worker location. Both paths use
published `antalya-rs =0.1.1`. The direct worker consumes/drops every Arrow batch;
the PostgreSQL path additionally converts tuples and runs its exact local qual.
EXPLAIN consumes those rows without serializing a million PostgreSQL protocol
rows to an application; application result transfer is outside this benchmark.
HTTP-only engine measurements are a third path and are not this direct client.

Use already owned PostgreSQL 18.6 and framing-capable Antalya fixtures with bearer
authentication and access to their genuine `system.query_log`. Export
`ANTALYA_TEST_URL`, `ANTALYA_TEST_TOKEN`, and optionally `FDW_ENGINE_URL` for a
container-visible address. Credentials are never stored in result artifacts.
The profiler uses fixed `fdw_profile*` objects in disposable fixtures. The PG
container must mount this checkout at `/workspace`; it uses `psql -U postgres`.

```sh
python3 tests/benchmarks/fdw_profile.py --pg-container OWNED_PG \
  --direct-worker /absolute/path/to/release/examples/fdw_profile \
  --output .local/fdw-smoke --setup --setup-only
```

Generate lake data in the native engine's mounted user-files directory using
`fdw_lake_fixture.py --directory SHARED/fdw_profile --cli WC_LAKE
--index-directory .local/fdw-index`. It requires the existing PyArrow test
dependency. The default remote prefix matches `benchmark/fdw_profile` under
ClickHouse's user-files directory. Load the exported `index.csv`, `files.csv`
and `grams.csv` into `wc_lake_index(index_name,column_name,views,snapshot_id)`,
`wc_lake_file`, `wc_lake_gram` with PostgreSQL `\copy`. Then create:

```sql
CREATE FOREIGN TABLE fdw_profile_lake_full(id bigint,url text,extra text)
SERVER fdw_profile OPTIONS(table 'file(''benchmark/fdw_profile/*.parquet'', Parquet)');
CREATE FOREIGN TABLE fdw_profile_lake_indexed(id bigint,url text,extra text)
SERVER fdw_profile OPTIONS(table 'file(''benchmark/fdw_profile/*.parquet'', Parquet)',
                          lake_index 'profile_lake');
```

Choose focused coverage with `--workloads`, `--policies`, `--budgets` and
`--concurrency`; each accepts a comma-separated list. Use a distinct output
folder per invocation. Cover narrow/wide projections and both lake workloads at concurrency one, all four
terminal policies on narrow scans, multiple budgets on wide scans, and
requested concurrency 1/8/32 on narrow/wide scans. Dispatch uses a thread pool
without a start barrier; requested workers are not proof of actual peak
simultaneous engine queries. Repeating every Cartesian combination
is generally unnecessary. Add `broad_recheck` to cover an OR of two wildcard
predicates matching all one million rows, with the exact PostgreSQL local qual
retained. `--timing-off` provides an executor-clock overhead control; conversion
time is intentionally zero in that mode. Every cohort runs the same generated remote SQL
through each route and correlates its unique query ID to the native query log.
Post-cohort `SYSTEM FLUSH LOGS` is outside the measured operation.
Worker processes have a 315-second host deadline. PostgreSQL sessions also use
a 300-second statement timeout and 10-second lock timeout. On failure, the
profiler reaps its process group and sampler and terminates only its uniquely
named PG session. It saves the failed cohort and stops; process cleanup is not
treated as proof of remote terminal completion, and no next cohort starts.

Raw JSONL includes server rows/bytes/time/memory/ProfileEvents, full PostgreSQL
EXPLAIN, client stage clocks, and 10 ms sampled process RSS/HWM/thread maxima.
One fresh client/backend per sample means connection/cache initialization is
included; process launch is separately reported. PostgreSQL `wall_ms` is its
EXPLAIN execution time; direct `wall_ms` starts at fetch. PostgreSQL EXPLAIN
`TIMING ON` adds instrumentation overhead. Small/short-lived processes may evade
sampling; missing samples are not zero memory. RSS is not a batch budget.
The summary keeps unavailable counters/resource peaks as JSON null or empty CSV
cells. Corresponding `*_samples` columns give the number of actual observations,
so partial coverage and measured zero remain distinguishable.
The first decoded batch is not HTTP TTFB, local recheck CPU is not isolated,
and overlapping phase clocks must never be added or subtracted as exclusive
stage accounting. See `settings.json` for these measurement contracts.

A real CPU capture can use `fdw_perf.py --pg-container OWNED_PG
--perf-container OWNED_PERF --output .local/fdw-perf`. The profiler container
must share only the owned PG container's PID namespace and have `CAP_PERFMON`,
`CAP_SYS_PTRACE` and a compatible installed `perf`. The qualification used a
scoped `seccomp=unconfined` profiler sidecar and changed no host sysctl. Mount the
checkout at `/workspace` and provide the matching extension binary at its PG
library path for symbolization. The script attaches to one backend, records
`cycles:u` at 199 Hz with DWARF stacks during five sequential million-row wide scans with the broad wildcard local recheck
using `in_band` and `TIMING OFF` to avoid per-row measurement clocks, and writes actual samples, EXPLAIN output, `perf report`,
folded stacks and an SVG flame graph. CPU samples are distinct from wall clocks;
blocked network and queue waits do not consume sampled CPU. Sampling overhead
and five executions mean this capture is separate from latency benchmark rows.

For actual PostgreSQL result delivery, `fdw_wire.py --pg-container OWNED_PG
--output .local/fdw-wire --repeats 3 --sink-location container` runs narrow and broad wildcard queries with
`COPY (...) TO STDOUT (FORMAT binary)`. `psql` connects over TCP loopback inside
the owned container; a Python sink there reads 1 MiB chunks and returns only
JSON through Docker. The inner process clock includes psql launch, PostgreSQL
protocol serialization and delivery, the FDW, and container-local consumption.
The outer Docker/Python process clock is separately reported. The optional
`--sink-location host` control forwards all COPY bytes through Docker and includes
that forwarding and host consumption in its process clock. This is a third
PostgreSQL measurement boundary, distinct from EXPLAIN executor time and direct
client fetch time. The first complete COPY-header offset is not HTTP TTFB.

Counted bytes are COPY payload, excluding PostgreSQL message/TCP headers.
No Python per-row parser or full-result buffering is used. The deterministic
oracle requires the exact 19-byte COPY header, the 2-byte EOF marker, successful
psql completion, and 14,000,021 bytes (one million integer rows) or 56,000,221
bytes (one million three-column broad-wildcard rows, with 100 longer URLs).
Each temporary foreign table embeds a unique harmless SQL comment so native
`system.query_log` can identify the exact FDW query ID after the measured copy.
The native result-row count must also be one million. Log flushing/correlation
and temporary-table creation/drop are outside the measured process clock. Raw
wire records use `raw-copy.jsonl` so the executor-summary exporter does not mix
these different clock boundaries. The container's `timeout` bounds psql lifetime; a host watchdog separately bounds
blocked Docker/stdout transport reads (10 seconds beyond that remote deadline).
Measured records are persisted before untimed controls, including cleanup errors.

For adaptive selection, `fdw_adaptive.py --pg-container OWNED_PG --direct-worker
PATH_TO_RELEASE_WORKER --output .local/fdw-adaptive` runs a selective wildcard
on the million-row MergeTree fixture. One fresh PostgreSQL backend runs automatic
selection twice, then a manual exact reference. The full ANALYZE plans retain
the actual chosen SQL, preparation probes, cache hits, decision and fallback
reason. Cold/warm labels refer only to the backend adaptive cache; server and OS
caches are not flushed. Three fresh direct clients execute the actual chosen SQL
from those plans, so controls never substitute a non-ANALYZE planning guess.
All six data operations must return 100 rows and correlate to native QueryFinish
records. `raw-adaptive.json` deliberately stays outside the primary matrix's
`raw.jsonl` summary input. Preparation clocks overlap their transfer/decode/terminal
components and cannot be subtracted into exclusive phases.
