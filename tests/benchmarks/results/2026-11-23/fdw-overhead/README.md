<!-- SPDX-License-Identifier: Apache-2.0 -->
# FDW overhead follow-up

Batch-prepared Arrow columns and constant matcher call sites reduce broad-recheck
work in this local run. **The 50% overhead target was not met:** the sampled
non-matcher user-CPU estimate fell 29.4%; total backend CPU fell 26.3% in the
separate profile. SQL signatures and exact local rechecks are unchanged.

| Workload | Backend executor: paired median reduction | Full COPY: paired median reduction |
|---|---:|---:|
| Narrow | 7.6% | 3.4% |
| Short text | -8.6% (slower) | 9.6% |
| 1 KiB wide | 6.7% | 12.0% |
| Broad wildcard recheck | 15.1% | 24.7% |

These are medians of five paired percentage changes, not ratios of pooled
medians. Ranges overlap; some individual pairs regress. The unchanged direct
client also varies, so this is not evidence of a general latency improvement.
[All observations, ranges, CPU attribution and limits](measurements.md).

**Local shared machine only; not yet a reproducible benchmark.** Same historical
conversion harness, remote projections, `in_band`, 16 MiB batch budget,
`max_block_size=1024`, and `max_threads=2`; five alternating pairs with excluded
warmups. Narrow, short-text and broad-recheck queries return 1M rows; the wide
case returns 32,768. Direct published antalya-rs 0.1.0, backend EXPLAIN TIMING OFF,
and complete TCP COPY delivery remain separate clocks. No Elastic result is
substituted for these different workloads.

Profiles use 20 scans each, exact saved release binaries, DWARF stacks and
`perf --no-inline`. Inlined matcher work cannot be fully separated; the CPU
estimate is a proxy, not an exclusive stage measurement. Raw DWARF memory dumps
remain private. [Settings and qualification](settings.json). The archive contains build
provenance and the exact baseline-to-candidate source patch.
The full stack passed 86 PostgreSQL 18.6 tests and 58 FDW integration assertions.

## Inspect or verify

[Baseline broad-recheck flamegraph](https://github.com/f1b0-gh/pg_wildcard/releases/download/benchmark-evidence-2026-11-23/fdw-overhead-baseline-broad-recheck.svg)
· [Optimized flamegraph](https://github.com/f1b0-gh/pg_wildcard/releases/download/benchmark-evidence-2026-11-23/fdw-overhead-optimized-broad-recheck.svg)
· [Complete evidence archive](https://github.com/f1b0-gh/pg_wildcard/releases/download/benchmark-evidence-2026-11-23/fdw-overhead-439aada360f5.tar.gz)

From the repository root, download by checksum and reproduce the summary without
running either database (requires the artifact tooling in stacked PR #48):

```sh
python3 tools/verify_benchmark_artifacts.py --manifest tests/benchmarks/results/2026-11-23/fdw-overhead/artifacts.json
```

Add `--offline` after the first download, or `--archive PATH` for an already
downloaded asset. `--keep` retains the verified extraction for inspection.
Every archived file is listed in [SHA256SUMS](SHA256SUMS); exact-value controls
are recorded attestations, while the raw COPY/Arrow payloads are not retained.
