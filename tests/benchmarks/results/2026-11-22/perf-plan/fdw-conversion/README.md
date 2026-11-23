<!-- SPDX-License-Identifier: Apache-2.0 -->
# Batch-scoped Arrow conversion experiment

Complete raw evidence and the original hash manifests are preserved in the
[checksummed release archive](../../../../ARTIFACTS.md). From the repository root,
run `python3 tools/verify_benchmark_artifacts.py` to verify this historical run.


Five fresh interleaved baseline/prepared release-binary rounds, each with an
excluded warmup. Both variants use `in_band` and the same 16 MiB batch allowance.
All clients use persistent connections. `direct` consumes every Arrow
batch through published antalya-rs 0.1.0; `pg_backend` executes EXPLAIN ANALYZE
with TIMING OFF; `pg_copy` completes binary COPY over TCP into a local file.
The direct client is an unchanged control. It cannot benefit from this FDW change.
Exact values are verified outside the clocks, including complete COPY output.

Narrow, small-text and broad-recheck workloads each return one million rows.
The 1-KiB-wide workload returns 32,768 rows to remain inside the result budget;
its time must not be presented as a million-wide-row result. See raw query
records for projections and exact predicates.

| Workload | Route | Baseline ms median [range] | Prepared ms median [range] | Paired change median [range] |
|---|---|---:|---:|---:|
| narrow | direct | 322.20 [308.15–512.12] | 409.63 [280.27–419.12] | -9.0% [-20.0%–+30.1%] |
| narrow | pg_backend | 345.53 [297.94–928.29] | 386.84 [298.26–644.08] | +0.1% [-58.3%–+65.1%] |
| narrow | pg_copy | 512.91 [404.93–768.82] | 639.94 [383.77–690.07] | -1.2% [-25.2%–+70.4%] |
| smalltext | direct | 1150.35 [1003.87–1280.94] | 1282.34 [1002.60–1385.27] | +11.5% [-5.1%–+23.2%] |
| smalltext | pg_backend | 1340.95 [942.85–1594.34] | 1611.14 [966.86–1949.41] | +11.9% [-20.5%–+25.9%] |
| smalltext | pg_copy | 1161.53 [1000.81–1745.47] | 1627.80 [1034.76–1806.54] | +16.2% [-9.6%–+52.0%] |
| wide1k | direct | 634.57 [573.37–1137.18] | 806.23 [595.43–961.78] | +3.3% [-29.1%–+67.7%] |
| wide1k | pg_backend | 665.54 [607.08–838.48] | 786.32 [594.60–930.37] | +11.0% [-10.7%–+24.6%] |
| wide1k | pg_copy | 705.06 [615.69–967.96] | 847.99 [613.99–1097.03] | +24.2% [-12.9%–+25.5%] |
| broad_recheck | direct | 1151.17 [924.28–1650.30] | 1021.67 [934.72–1591.23] | +1.1% [-31.1%–+15.8%] |
| broad_recheck | pg_backend | 1115.21 [737.74–1416.16] | 884.27 [776.78–1251.20] | -18.7% [-29.0%–+7.5%] |
| broad_recheck | pg_copy | 2044.51 [1311.70–2279.85] | 1544.01 [1124.87–1914.75] | -16.0% [-34.2%–+17.7%] |

The original `flush_first` comparison stopped during its third baseline round:
the direct client refused a wide-row batch with a data-phase resource limit.
The prior completed and partial samples remain under `earlier-flush-first/` and
are excluded from this fresh comparison. An isolated reproduction failed on
its fourth read at 16 MiB. A separate 32 MiB direct-only diagnostic completed
40 reads, two with charged batches above 16 MiB; the maximum was 17,142,712 bytes.
That larger-budget probe is not a latency substitute or a production FDW change.
The `in_band` 16 MiB stability probe completed 40 reads, with a largest charged
batch of 8,668,568 bytes. See [resource-refusal.json](../../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-22/perf-plan/fdw-conversion/resource-refusal.json`).
This experiment does not establish reliable wide-row `flush_first` delivery.

The optimization decision and its rationale are in [decision.json](decision.json).
The host is shared and busy; ranges and unchanged-route variation are retained.
These are warm observations with uncontrolled filesystem caches, not cold tests.
Native server time, producer waits, decode, terminal confirmation and backend
execution overlap. [stages.json](../../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-22/perf-plan/fdw-conversion/stages.json`) exposes them as diagnostics, not
additive exclusive stages. TIMING OFF intentionally omits per-row conversion
timers. First-batch time is not network time to first byte.

Each trial includes compressed raw records, query IDs, query-log profiles,
settings and exact SQL. Backend sampled RSS and thread count are separate from
the largest individual BudgetedBatch charge. RSS is not bounded by that charge.
The runner snapshots reproduce fixture construction, complete-value controls,
sampling and timer boundaries; private fixture tokens are not included.

## CPU profiles

`flames/{baseline,prepared}/{narrow,smalltext,wide1k,broad_recheck}/` contains
real SVG flamegraphs, folded symbols, perf reports and exact workload settings.
These are separate profiling runs: 20 scans per workload with user CPU events,
199-Hz sampling and DWARF unwinding. Their terminal policy is `in_band`, matching
the fresh latency trials. Do not substitute profiled times for latency samples.
Widths are inclusive sample attribution, not exclusive wall-time percentages.
Each profile records the exact release-binary hash used in the latency trial.
Raw DWARF memory dumps remain private; their hashes are retained for provenance.

The first symbolization left extension frames unresolved. The same recordings
were processed again using stable symbol directories and the exact saved binary
for each variant; no query was replayed. All eight sample totals stayed unchanged.
[Symbolization provenance](../../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-22/perf-plan/fdw-conversion/flames/symbolization.json`) records the mappings,
postprocessor hash and remaining unresolved frames. The
[CPU attribution table](flames/cpu-attribution.md) describes overlapping stack
categories and their limits. Original unresolved derivatives remain private.

The conversion prototype was rejected and removed. Its
[patch](../../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-22/perf-plan/fdw-conversion/rejected-prototype.patch`) is retained for reproduction and review,
not as an enabled optimization. Untimed full-value controls precede direct-client
timings on every connection; measured direct runs consume every batch and check
row counts. Every measured COPY validates every field and the complete unique
row-ID set after its timer. Backend scans verify complete row counts.

The baseline module snapshots match published parent
`ae33a14c923ea16251c756793cbc84c6a754553b`; module and binary hashes are recorded
separately. This does not attest that the entire later benchmark working tree
was the input of either earlier binary build. Existing follow-ups cover
[conversion](https://github.com/f1b0-gh/pg_wildcard/issues/17),
[streaming](https://github.com/f1b0-gh/pg_wildcard/issues/18),
[terminal proof](https://github.com/f1b0-gh/pg_wildcard/issues/19) and
[adaptive preparation](https://github.com/f1b0-gh/pg_wildcard/issues/20).
