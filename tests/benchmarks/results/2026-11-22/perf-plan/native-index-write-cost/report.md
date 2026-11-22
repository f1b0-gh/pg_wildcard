# Bounded byte-index write and storage cost

Five paired, alternating-order trials on fresh tables. Each table receives 900,000 initial rows, then 100,000 incremental rows. Native HTTP request clocks include INSERT execution and completion; server durations are separate diagnostics. The text index is maintained during insertion, so these are write costs, not timings of a separate backfill operation. No antalya-rs, PostgreSQL or Elastic route is measured here.

| Layout | Insert rows | Request seconds median [range] | Native server ms median [range] | Native CPU ms median [range] | Query peak MiB median [range] |
|---|---:|---:|---:|---:|---:|
| plain | 900000 | 0.586 [0.443, 0.667] | 582.000 [441.000, 664.000] | 559.660 [483.323, 584.031] | 13.289 [12.228, 15.712] |
| plain | 100000 | 0.076 [0.064, 0.104] | 73.000 [62.000, 90.000] | 69.814 [60.542, 71.336] | 9.159 [9.159, 9.159] |
| byte_index | 900000 | 3.881 [3.571, 4.830] | 3873.000 [3567.000, 4826.000] | 3745.085 [3475.192, 4318.968] | 51.773 [50.715, 52.829] |
| byte_index | 100000 | 0.486 [0.392, 0.573] | 483.000 [388.000, 565.000] | 455.868 [383.502, 505.646] | 46.755 [46.755, 46.755] |

All ten full one-million-row corpus hashes agree. Tables were settled with OPTIMIZE FINAL before storage inspection. Source-data compressed bytes are identical, which makes the additional compressed index bytes explicit.

| Layout | Total bytes median | Source compressed bytes median | Index compressed bytes median |
|---|---:|---:|---:|
| plain | 9,383,542 | 9,375,381 | 0 |
| byte_index | 15,554,355 | 9,375,381 | 6,170,663 |

The incremental insertion median is 6.40× the plain layout in this fixture. This is a material tradeoff even where the separate query experiments improve latency. It does not establish useful file/mark pruning or justify enabling the prototype for every field.

The bounded expression indexes raw three-byte substrings, retaining an overflow sentinel for values beyond its cap. Exact DDL and per-trial host load are in raw.jsonl. This private prototype remains separate from the production text-index layout and the opt-in native watchlist join.

Provenance limitation: the original private runner did not record a dedicated start-of-run machine/settings snapshot or query IDs. All twenty original INSERT QueryFinish records, including their IDs, settings and counters, were recovered from the owned server after the host reboot; no timings were replayed. The recovery file explicitly labels its machine and source-hash observations as post-run. Query-log settings are the engine-recorded map, not a complete snapshot of every default. A later host snapshot cannot prove the earlier configuration. The original raw clocks and summary are preserved.

Reproduce the report (no engine calls): `python3 .local/perf/indexed/report-write-cost.py`.
