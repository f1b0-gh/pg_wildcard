<!-- SPDX-License-Identifier: Apache-2.0 -->
# FDW streaming, completion and adaptive-cache controls

This campaign addresses issues #18–20. PostgreSQL continues using published
antalya-rs **0.1.0**. A separate modified-client experiment evaluates the opt-in
Zstd option in [antalya-rs #14](https://github.com/f1b0-gh/antalya-rs/pull/14);
it is not a published-release or PostgreSQL result.

## Methodology and scope

All fixtures are synthetic. Antalya 26.8.34.20001.altinityantalya runs with
4 CPUs and 8 GiB, PostgreSQL is 18.6. Machine details, exact binary/source hashes,
SQL, settings, native query IDs and ProfileEvents accompany each cohort.
The host is shared and had unrelated load; raw observations retain load averages.
Five measured rounds follow one excluded value-check warmup per configuration.
Workloads are grouped; configuration order rotates within each workload.
Indexed/full latency medians are descriptive, not randomized causal estimates. Cache trials deliberately include cold operations
and alternate the order of identically built baseline/candidate binaries across
five pairs. No task-owned build or test ran during timing.

Routes are separate: raw native HTTP Arrow, direct published client with confirmed
completion, PostgreSQL `EXPLAIN (ANALYZE, TIMING OFF)` request, and full binary COPY.
COPY includes transfer of every PostgreSQL value to the client. Direct measured
operations drain every batch and require confirmed completion. Complete fields
and unique ID sets are checked inside excluded direct warmups, and outside the
request clocks on every raw HTTP/COPY request. Historical settings metadata
broadly says verification is outside clocks; the direct warmup exception here
corrects that wording without changing the original records. PostgreSQL checks remote/output counts, complete
metrics and zero local rejection. A zero conversion clock with TIMING OFF means
unmeasured. Native HTTP is a format/server control, not proof-equivalent to the
client's terminal policies; its policy label identifies scheduling only.

Source data are one million narrow IDs or three small-text columns; the wide
case has **32,768 rows**, each with a repetitive 1 KiB text field. These are
transport tests, not the Elasticsearch rule-search comparison in the main README.
Do not divide timings across those different workloads.

Clocks overlap: server duration includes output scheduling; client body wait is
not pure network latency; producer queue wait overlaps backend consumption.
ProfileEvents can aggregate work across threads. The native [counter definitions](https://github.com/ClickHouse/ClickHouse/blob/master/src/Common/ProfileEvents.cpp) describe send/wait and read-syscall time; the latter includes page-cache reads. No exclusive-stage subtraction,
TTFB inference from first-batch offsets, or backend-RSS bound is made here.
The default output-batch cap remains 16 MiB and the retained-client budget 64 MiB.
Requested `max_block_size` is not a hard Arrow batch size.

## Streaming and framing (#18)

For one million narrow rows with `flush_first`, the 1,024/4,096/16,384 block
requests gave direct medians **214.020/93.424/71.074 ms**, backend-request medians
**316.518/274.697/278.074 ms**, and complete COPY medians
**550.834/462.507/503.067 ms** (see exact values in the generated tables).
The same direct samples show body-wait medians 147.496/48.574/28.329 ms and
server durations 177/57/30 ms. Native send/wait counters fell from 92.275 to
12.643 ms while read-syscall time stayed around 1.7–1.9 ms. These observations
support reduced output/scheduling work with larger blocks, not a pure network
bottleneck attribution. Arrow decode also fell from 12.173 to 3.015 ms.

Larger requested blocks reduce the direct-client overhead in the repeated
narrow/small-text controls. Complete COPY gains vary, so choose settings using
the full route that matters to the application. No global block-size default
or result budget changes. The 16,384-row plain wide PostgreSQL warmup refused
the configured resource limit; `streaming-wide/resource-refusal.json` preserves
that result. It has no completed repeated cohort. The separate wide in-band
cohort completed all configurations within the same output budget.

`tables.md` reports server duration alongside body wait, decode, framing,
terminal controls, network send/wait, read-syscall and CPU counters. The latter
are observations, not a complete partition of time. Prior exact-binary
[CPU profiles and flamegraphs](../fdw-conversion/) identify inclusive Base64 and
Antalya framing work. The opt-in compression experiment tests a concrete way of
reducing framed bytes without changing regex, completion proof or raw values.

The original `streaming-full` native HTTP control inherited the server's Arrow
compression default (observed LZ4), whereas direct/PG requested none. It is
**excluded from matched uncompressed route comparisons**. `native-uncompressed`
is a separate explicit-none control, not a retrospective paired sample.
That first cohort also hit a post-measurement SQL-normalization bug and a cleanup
syntax error. Original failure/raw records and explicit recovery receipts remain;
all 288 requests were correlated from genuine query logs without replaying them.

| Modified direct client, 4,096 requested rows | None median ms | Zstd median ms | None body MB | Zstd body MB |
|---|---:|---:|---:|---:|
| Narrow, flush_first | 115.765 | 195.560 | 8.035 | 1.068 |
| Narrow, in_band | 223.377 | 152.071 | 10.743 | 1.450 |
| Small text, flush_first | 239.498 | 375.747 | 50.070 | 5.856 |
| Small text, in_band | 991.234 | 371.720 | 66.824 | 7.845 |
| 1 KiB text, in_band | 642.348 | 67.080 | 46.252 | 0.281 |

Body MB uses decimal millions and includes framing where applicable, not TCP/TLS
wire captures. Full distributions and decode costs are in the generated tables.

The modified-client compression experiment pairs none/Zstd in the same binary,
with five rotating rounds and identical SQL/block/output budgets. Plain cases
regressed while framed cases improved. Highly repetitive wide payloads exaggerate
compression gains. Zstd additionally reserves 32.5 MiB per pending read, versus
16 MiB for none, within the same 64 MiB pool; that changes concurrency admission.
It does not justify enabling compression universally.

## Terminal confirmation (#19)

Both cadence cohorts completed 288 requests. Every indexed query read one file
and evaluated 125,000 Parquet rows; every full query read eight files and evaluated
1,000,000 rows. Both returned the same 125 IDs and values. Native `read_rows`
counts after filtering here, so it alone would hide the eightfold difference.

At the 1,000 ms log cadence, median complete-operation times were:

| Workload | Direct flush_first | Direct in_band | PG COPY flush_first | PG COPY in_band |
|---|---:|---:|---:|---:|
| Small result | 38.256 ms | 6.739 ms | 38.539 ms | 8.182 ms |
| Indexed lake | 48.520 ms | 20.882 ms | 52.603 ms | 21.459 ms |
| Full lake | 56.413 ms | 31.734 ms | 57.645 ms | 29.533 ms |

For the small result, direct `observe_only` fell from 1,056.319 ms at the
1,000 ms cadence to 117.364 ms at 100 ms. Indexed/full direct medians changed
from 1,089.572/1,055.349 ms to 98.361/219.991 ms. Log publication clearly matters,
but other policies were slower in the second cohort; shared-host load and
more frequent log work prevent assigning all differences to one cause.

For these short scans on a framing-capable Antalya server, explicitly selecting
`terminal_policy 'in_band'` avoids separate log-confirmation requests and was
fastest in both cohorts. Keep `flush_first` as the general default. In-band
responses add framing work and bytes on broad scans, and require the server's
compatible completion protocol. Choose `observe_only` when its publication-wait
tradeoff is acceptable; lowering the server's log interval has operational costs
and is not a blanket recommendation. None of these choices accepts EOF as proof.

ClickHouse documents asynchronous query-log publication through
[`flush_interval_milliseconds`](https://clickhouse.com/docs/reference/system-tables/query_log)
and explicit [`SYSTEM FLUSH LOGS`](https://clickhouse.com/docs/reference/statements/system).
The two cadence cohorts change only the owned fixture setting, with a restart
outside timing and restoration receipt. They are separate cohorts, not randomized
per-query cadence pairs. Diagnostic log collection runs after the entire cohort;
flushes required by a terminal policy remain part of its measured operation.

Flush/observe clocks are published-client aggregates per operation. Where a record
has one flush and one poll they describe those individual requests; multiple-poll
records retain totals and counts, not invented per-poll timings. PostgreSQL exposes
coarser terminal diagnostics. No HTTP EOF or provisional Arrow batch becomes success.

## Adaptive preparation (#20)

The bounded preference cache now promotes hits for eviction without changing the
60-second insertion lifetime, 32-entry capacity, 64 KiB key limit or client/SQL/
settings identities. In every paired hot/cold round, eight reused keys interleaved
with forty new keys improved from 80% to 100% hot hits and from 16 to zero extra
hot-key probes. This is not a claim about uniform cycles larger than capacity.

Persistent backends amortize probes; new backends do not share preferences.
The real 30-second/61-second checks confirm reuse before expiry and new probes
after original insertion expiry, even after an intervening hit. Settings changes
miss, and returning to the original settings reuses the original preference.
Preferences are not results: exact predicates, rechecks and completion evidence
remain required on every data operation. Existing hostile transport, cancellation,
unknown-plan and authentication-failure tests remain in the qualification suite.

## Reproduce and inspect

`python3 verify.py` verifies every payload hash and exact file membership, checks
full cohorts, row/completion/value contracts, actual file pruning and cache
boundaries, then regenerates [tables.md](tables.md) and [report.json](report.json)
offline. Run from this directory or use the script's absolute path. Gzip payloads
retain original selected synthetic observations; `provenance.json` records their
uncompressed source hashes. Private fixture state, credentials and diagnostics
are excluded. Detailed working notes remain in `.local/`.

The live runners are `tests/benchmarks/fdw_tuning.py`, `fdw_terminal.py` and
`fdw_cache.py`. Use `--help` for explicit ownership-checked fixture parameters;
[benchmark setup](../../../../README.md) covers the persistent workers. Retained
source snapshots identify measured versions. The compression worker's Cargo
manifest uses a local upstream path; point it at revision
`095fd14d0aa65d9ef4e48b0140d4891af8ac05d8` of antalya-rs to reproduce that separate
experiment. It does not replace any public consumer dependency.
