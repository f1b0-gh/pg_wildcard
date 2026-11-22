<!-- SPDX-License-Identifier: Apache-2.0 -->
# Native FDW profiling — 2026-11-22

384 successful, terminal-confirmed operations correlate by query ID with genuine Antalya `system.query_log`. PostgreSQL 18.6 uses the published `antalya-rs =0.1.0` client and native Antalya 26.8.34.20001. Both Rust paths are release builds. The native endpoint is local Docker-bridge HTTP; this is not a WAN/TLS benchmark. The native server has 4 CPUs / 8 GiB; PostgreSQL has no container-specific CPU/RAM cap. Exact machine/build/binary metadata is in [environment.json](environment.json).

## Route comparison without per-row timing

PostgreSQL numbers use `EXPLAIN ANALYZE TIMING OFF`, which consumes all rows, converts tuples and retains exact local quals, without transferring a million PostgreSQL protocol rows to an application. The direct client consumes/drops Arrow batches. Its wall clock starts at fetch; PostgreSQL execution time also includes FDW begin/client setup. Planning and process launch are separately recorded. These are useful route comparisons with different start boundaries.

| Workload | Returned rows | Direct ms | PostgreSQL ms | Direct rows/s | PostgreSQL rows/s | PG peak RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| One integer column | 1,000,000 | 212.7 | 381.2 | 4,701,789 | 2,623,233 | 26.09 |
| Three small columns (~42 value bytes/row) | 1,000,000 | 366.1 | 639.2 | 2,731,472 | 1,564,434 | 26.59 |
| Three columns + broad wildcard local recheck | 1,000,000 | 191.3 | 916.4 | 5,226,927 | 1,091,222 | 31.81 |
| Three columns, 1 KiB text payload | 32,768 | 105.5 | 124.0 | 310,509 | 264,307 | 31.41 |

The true-wide row uses a 1,024-byte `extra` value and 32,768 rows to stay within the unchanged 64 MiB server result cap. It has two paired repetitions; the other OFF baseline rows have three pairs, including two later repetitions. Variability is substantial: these are small-sample fixture medians, not stable production estimates. Rows/s is returned row count divided by median wall time, consistently across summary groups. This is a deterministic synthetic fixture, not a production workload distribution. Cohorts run direct first, then PostgreSQL; no OS cache flush or randomized-order claim is made.

Observed OFF latency ranges (min–max milliseconds; the first three rows have three samples per route):

| Workload | Direct fetch range ms | PG executor range ms |
|---|---:|---:|
| narrow | 159.7–215.5 | 217.6–409.7 |
| wide | 267.4–531.9 | 437.2–823.8 |
| broad_recheck | 121.1–191.4 | 575.9–971.3 |
| wide1k | 97.8–113.3 | 118.9–129.0 |

## PostgreSQL protocol delivery

Twelve supplementary controls delivered the complete binary COPY result through psql over TCP loopback: three repetitions per workload with a container-local 1 MiB chunk sink, and three with Docker stdout forwarding to a host sink. Only small JSON records cross Docker in the container-local mode. No per-row Python parser or full-result buffer is used. These controls are separate from the 384 paired executor/client samples.

| Workload | Rows | Direct process median ms (n=3) | PG executor OFF median ms (n=3) | Container-local psql median ms (n=3) | COPY payload bytes |
|---|---:|---:|---:|---:|---:|
| narrow | 1,000,000 | 231.5 | 381.2 | 693.3 | 14,000,021 |
| broad_recheck | 1,000,000 | 208.6 | 916.4 | 1813.0 | 56,000,221 |

The direct process reference includes process/client initialization and its sampling lifecycle, consuming/dropping Arrow batches. The inner COPY clock includes psql initialization, PG protocol serialization and container-local result consumption. Outer Docker/Python medians were 989.3/2052.6 ms (narrow/broad), separately retained. These different routes/formats and nonidentical initialization costs cannot be subtracted into an exclusive network or serialization cost. This is not a persistent-connection pool benchmark. Native server medians inside internal COPY were 189/1543 ms and overlap delivery/backpressure.

The earlier host-forwarded controls measured 717.9/1561.5 ms including Docker result forwarding and host consumption. They ran in a different window; the broad host-forwarded median being lower does not establish that forwarding helps. Small samples and run-to-run variability limit such attribution. See [internal raw records](wire-internal/raw-copy.jsonl), [internal summary](wire-internal/summary.json), [internal settings](wire-internal/settings.json), and [forwarded control records](wire/raw-copy.jsonl).

All twelve controls passed the exact 19-byte COPY header, 2-byte EOF trailer, deterministic payload length, successful psql exit, and uniquely tagged native query-log row count. COPY bytes exclude PG message/TCP headers. Internal first complete COPY-header offsets were 113.1/128.3 ms (narrow/broad), not HTTP TTFB. The public [wire harness](../../../fdw_wire.py) preserves both clock boundaries.

## Adaptive preparation and cache

A supplemental fresh backend ran `candidate_columns='url', candidate_mode='auto'` twice for `wc_match(url,'*powershell*','raw')`, then a manual exact reference. All returned 100 rows. Cold/warm refers to the backend adaptive cache only; server/OS caches were not cleared.

| Case | PG executor ms | Preparation ms | Probes | Cache hits | Direct actual-SQL fetch ms |
|---|---:|---:|---:|---:|---:|
| Automatic, cold | 202.829 | 77.394 | 2 | 0 | 85.323 |
| Automatic, warm | 78.291 | 0.003 | 0 | 1 | 73.199 |
| Manual exact | 87.646 | 0 | 0 | 0 | 70.723 |

Automatic selection conservatively retained exact SQL with `insufficient_mark_reduction`; this fixture does not demonstrate a beneficial candidate selection. Both observed probes were cached for the second execution. Cold preparation is 38% of the measured execution, including 63.451 ms nested terminal confirmation; these clocks are not additive. One sequence demonstrates cache behavior, not a robust latency distribution. The three direct controls executed the actual selected SQL retained in ANALYZE, identical across these cases, rather than a non-ANALYZE estimate. All six data query IDs correlate to native QueryFinish records. See [raw plans/counters](adaptive/raw-adaptive.json), [summary](adaptive/summary.json), and [adaptive harness](../../../fdw_adaptive.py).

## Timing instrumentation changes the workload

`pg_test_timing` measured **1,388.94 ns per timing loop** ([raw calibration](cpu/pg-test-timing.log)). One-million-row narrow scans took about 6 seconds with TIMING ON, versus 381.2 ms OFF (three-sample median). Broad wildcard scans were 6.33 seconds ON (two samples) versus 916.4 ms OFF (three-sample median). ON rows remain diagnostic records, not production-throughput evidence. Their per-row conversion clocks also include measurement overhead. A zero conversion counter in OFF rows means timing is disabled, not that conversion costs zero.

## Client and executor clocks

Example OFF broad-wildcard pair; times are milliseconds. These clocks overlap and are **not additive exclusive stages**. HTTP body wait includes server execution, transport and scheduling. Producer queue wait overlaps PostgreSQL consumer work. First batch is an offset to the first decoded Arrow batch, not HTTP time-to-first-byte.

| Route | Wall | First batch | HTTP body wait | Arrow decode | Terminal proof | Consumer wait | Producer queue wait |
|---|---:|---:|---:|---:|---:|---:|---:|
| direct_antalya_rs | 121.134 | 6.609 | 64.944 | 30.356 | 18.623 | — | — |
| postgres | 575.91 | 13.409 | 49.752 | 34.792 | 28.584 | 38.31 | 428.554 |

True HTTP TTFB, separately timed local recheck CPU, and simultaneous shared-client retained-byte peaks are not exposed by the published API / PostgreSQL callbacks used here. They are not inferred by subtracting overlapping clocks. Complete per-operation clocks, wire bytes, generated SQL, EXPLAIN and server counters are retained in each `raw.jsonl`; [summary.csv](summary.csv) and [summary.json](summary.json) aggregate them.

## Concurrency and budgets

OFF cohorts use separate client processes / PostgreSQL backends. Entries are median milliseconds per operation; 8/32 are requested thread-pool dispatch concurrency, with no start barrier. Actual peak simultaneous engine queries was not measured. The one-worker baseline has three repetitions; the 8/32 settings each have one dispatch cohort, not repeated independent cohorts.

| Projection | Requested workers | Direct ms | PostgreSQL ms | PG max RSS MiB | PG max threads/backend |
|---|---:|---:|---:|---:|---:|
| narrow | 1 | 212.7 | 381.2 | 26.09 | 2 |
| narrow | 8 | 460.7 | 602.3 | 28.45 | 2 |
| narrow | 32 | 1827.1 | 1781.8 | 26.91 | 2 |
| wide | 1 | 366.1 | 639.2 | 26.59 | 2 |
| wide | 8 | 1008.6 | 1075.8 | 26.70 | 2 |
| wide | 32 | 4642.5 | 3530.8 | 28.29 | 2 |

RSS/HWM/threads are sampled from `/proc` every 10 ms, so short peaks can be missed. RSS includes shared mapped pages and must not be summed as unique physical memory. The observed backend had two threads (backend plus one Tokio worker).

**The proposed `backend RSS <= batch budget` acceptance condition is false.** With a 1 MiB batch limit the three-small-column OFF scan retained an observed individual batch of 103224 bytes, while backend RSS peaked at 26.691 MiB. The 1/4/16 MiB budgets were all above these small batches; this is not a stress test of the shared 64 MiB client budget. The true-wide fixture reached 2150840 charged bytes in one batch under the 16 MiB limit.

`batch_bytes` bounds each returned batch charge / pending-read reservation. The client separately shares a 64 MiB retained-byte budget. Current, queued and pending-send batches, decode buffers, PostgreSQL tuple copies, code mappings and allocators have different owners. The measured peak is one individual `BudgetedBatch`, not a simultaneous aggregate and not an RSS limit. All observed individual charges respected the configured limit. A separate [negative control](budget-rejection.json) configured 1 MiB for the genuine 1 KiB-column fixture and correctly failed with a resource-limit error; its option change was rolled back.

## Terminal policies

OFF narrow scan, three pairs for flush_first and one pair for each other policy; terminal confirmation is nested within the operation.

| Policy | Direct wall ms | Direct terminal ms | PG wall ms | PG terminal ms |
|---|---:|---:|---:|---:|
| flush_first | 212.7 | 29.6 | 381.2 | 29.2 |
| observe_first | 182.7 | 42.3 | 241.9 | 45.6 |
| observe_only | 380.2 | 234.4 | 854.9 | 670.4 |
| in_band | 254.3 | 0.0 | 333.0 | 0.0 |

`in_band` used zero data control requests but increased server/body work in this fixture; removing query-log confirmation did not automatically lower total latency. `observe_only` exposed asynchronous query-log delay. These single cohorts are evidence of tradeoffs, not a universal policy ranking.

## Real lake pruning

Both paths returned exactly the same deterministic 125 IDs, also checked against a full native scan. The index excluded 7 of 8 known files while retaining the conservative unknown-file fallback. Native `EngineFileLikeReadFiles` fell from **8 to 1**, and `ParquetRowsFilterExpression` from **1,000,000 to 125,000**. Native `read_rows` was 125 for both: it counts rows after this Parquet predicate and cannot establish scanned-file work. Highly repetitive benign files compress well, so file-count reduction is not an eightfold byte/latency claim.

These two-repetition selective measurements use TIMING ON for only 125 returned rows. Direct indexed/full medians were 137.4/77.1 ms while server duration was 13.5/29.0 ms: terminal confirmation (119.6/44.6 ms) masked the lower file-scan work. PostgreSQL indexed/full medians were 114.2/131.7 ms. Do not claim an unconditional end-to-end index speedup from this fixture.

## Actual CPU profile

[Flame graph](cpu/flamegraph.svg), [folded samples](cpu/flamegraph.folded), [demangled perf script](cpu/perf-demangled.script), [perf report](cpu/perf-report-demangled.txt), and [five EXPLAIN records](cpu/perf-explain.json) come from five sequential million-row broad-wildcard scans, TIMING OFF, `in_band`. A task-owned profiler sidecar shared only the owned PG PID namespace and used scoped PERFMON/SYS_PTRACE capabilities; host perf sysctls were unchanged.

`perf record` collected **849 cycles:u samples, zero lost**. The perf report attributes 42.47% inclusive weighted user-space cycles to `iterate_foreign_scan_inner`, including 35.00% in the memory-context conversion region and 20.79% in `values::input`. The `wc_match` wrapper is 29.42% inclusive. These nested percentages are not additive. The flame graph widths use sample counts; the perf report weights event periods. CPU samples do not measure blocked network wait.

The raw 13.499 MiB DWARF `perf.data` remains under `.local/perf-review/fdw/cpu/` because stack-memory dumps warrant private handling; symbolic sample logs are retained here. Its SHA-256 is `f6b18b7cccc2d57b7475f519f65d3c4506fc9099234d878d2fb2d4fb35788aa4`.

Evidence-based follow-ups:

- [#17: specialize batch conversion while preserving validation/ownership](https://github.com/f1b0-gh/pg_wildcard/issues/17).
- [#18: attribute server/body streaming wait before tuning](https://github.com/f1b0-gh/pg_wildcard/issues/18).
- [#19: investigate terminal confirmation masking selective pruning gains](https://github.com/f1b0-gh/pg_wildcard/issues/19).
- [#20: measure and amortize cold adaptive preparation under backend/cache churn](https://github.com/f1b0-gh/pg_wildcard/issues/20).

Producer backpressure is not a separate optimization target merely because its clock is large.

## Reproduction and qualification

See [the harness guide](../../../FDW.md). Regenerate the CSV/JSON with the public [summary exporter](../../../summarize_fdw.py): `python3 tests/benchmarks/summarize_fdw.py tests/benchmarks/results/2026-11-22/fdw`. The original attempt leaked Docker-exec samplers and was discarded entirely; only the clean sentinel-controlled rerun is retained here. Normal completion was verified to leave no sampler processes. Signal/lease lifecycle hardening is a later harness reliability change, outside the measured data path.

All 384 operations have a genuine QueryFinish record, expected row count, complete FDW metrics where applicable, and zero rows removed by PostgreSQL local quals. Exact selective ID equality was checked separately. Final checks passed: 40 standalone FDW harness tests, harness clippy with warnings denied, 68 PostgreSQL 18.6 pgrx tests, 11 sampler/control tests, three supplemental helper tests, owned Rust formatting, and a native 125-row profiler smoke against the current test build. The first pgrx attempt lacked USER in its container environment; the corrected USER=review retry passed. Benchmark release binaries were preserved before the test installation and are identified by hashes in environment.json. This benchmark does not qualify PostgreSQL majors 14–17.

The rendered perf report and demangled script have trailing spaces removed for version control; sample records and folded stacks retain the measurements.
