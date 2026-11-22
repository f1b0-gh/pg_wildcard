<!-- SPDX-License-Identifier: Apache-2.0 -->
# Bounded byte-index write costs

Five paired, alternating-order trials on fresh MergeTree tables, each receiving
900,000 initial rows followed by 100,000 incremental rows. Every completed table
has the same one-million-row corpus digest. OPTIMIZE FINAL precedes storage
inspection. This measures native HTTP INSERT execution; it does not measure
direct antalya-rs, PostgreSQL, Elastic or a regex-engine replacement.

| Layout | Initial 900k rows, median seconds | Incremental 100k rows, median seconds | Compressed index bytes |
|---|---:|---:|---:|
| Plain | 0.586 | 0.076 | 0 |
| Bounded byte-array text index | 3.881 | 0.486 | 6,170,663 |

The indexed incremental median is 6.40 times the plain layout. Total settled
storage is 9,383,542 versus 15,554,355 bytes, with identical 9,375,381 compressed
source-data bytes. See [the full report](report.md) for ranges, native CPU,
server time and query memory. These costs are a reason to evaluate the index
selectively. This evidence does not establish useful physical read pruning or
enable a production indexed-dispatch API.

The index uses raw three-byte substrings, with bounded extraction and an overflow
sentinel. Exact DDL is in raw.jsonl.gz. This is a private index-layout experiment,
separate from the supported materialized-view/text-index layout and the opt-in
native watchlist join. Query latency belongs in a separate experiment.

## Evidence and limits

- raw.jsonl.gz retains original request clocks, DDL, storage, corpus digests and
  host load. summary.json is the original summary; report.json validates it.
- recovered-profiles.json.gz contains all 20 original INSERT QueryFinish records,
  exact query IDs, SQL, settings and CPU/memory counters. They were recovered
  after a host reboot without replaying the measurements. The write-cost run
  itself completed before the unrelated C32 run was interrupted.
- The original private runner omitted a start-of-run machine/settings snapshot.
  Recovery machine and source-hash observations are explicitly post-run. The
  query-log settings map is not a complete snapshot of engine defaults. No full
  historical build/configuration attestation is claimed.
- The five trials are not independent hosts. Filesystem caches were uncontrolled;
  host load is recorded. CPU counters, query memory and wall clocks measure
  different things and must not be added or treated as backend RSS.

## Reproduction

reproduction-layout.json maps exact source/specification snapshots back to their
repository-relative locations. Restore those files in an isolated checkout,
decompress raw.jsonl.gz and recovered-profiles.json.gz into
.local/perf/indexed/write-cost/, and copy summary.json there. Running
`python3 .local/perf/indexed/report-write-cost.py` regenerates and validates the
report without contacting an engine. The reporter also requires the recovered
profile file; it does not substitute current settings for the missing snapshot.

To repeat the experiment rather than the report, retain the original results
elsewhere, provision an owned native fixture using the repository benchmark
instructions, provide its private .local/perf/antalya.json, and run
`python3 .local/perf/indexed/index-write-cost.py` from the repository root. It
requires a fresh output directory, existing benchmark helpers and the supplied
unrelated-1000 specification. It refuses to overwrite this evidence. Engine limits
are set by libwildcard/tests/benchmarks/sweeps.py. The snapshots were collected
after the completed run; their timing provenance is not retroactively asserted.

sha256.json covers all payloads; compressed files use deterministic gzip. Private
fixture state, credentials and endpoints are excluded.
