# Watchlist helper-alias correctness fix

Complete raw evidence and the original hash manifests are preserved in the
[checksummed release archive](../../../../ARTIFACTS.md). From the repository root,
run `python3 tools/verify_benchmark_artifacts.py` to verify this historical run.


When ClickHouse prefers column names to aliases, a hidden source column named
`_wc0_value` could supply the candidate keys while the exact matcher checked the
real value. For example, raw `v = 'abc'`, hidden `_wc0_value = 'xyz'` and a rule
requiring `abc` could lose a match. The renderer now isolates the selected fields
and raw value in an inner projection before referencing its helper aliases.

The regression corpus failed nine checks before the fix. The six qualification
logs cover seeds 1, 42 and 1234 on chDB and native Antalya, including both values
of `prefer_column_name_to_alias`, five projection forms and seven materialized-view
lifecycle phases. They compare complete hit multisets and projections.

[The transfer report](report.md) compares the previous production renderer with
this fixed renderer on identical shared-literal and unrelated fixtures. Both
have one million rows, 1,000 distinct rules and exactly 10,000 hit pairs.
Five interleaved pairs follow warmup; all pairs match. Median changes are −3.11%
and +0.22%, respectively. Neither exceeds the 10% regression investigation gate.
This is a correctness fix with a performance transfer check, not a new speedup
claim or an Elastic comparison. Original historical measurements remain intact.

`measurements/` preserves raw request/profile records, settings, machine details,
SQL variants, corpus hashes, summaries and cleanup results. `diagnostics.json`
uses the inherited key `prototype` for the **previous production renderer**;
`production` identifies the fixed renderer. Both source hashes are recorded.
`specs/` contains runner inputs, including the baseline SQL; measured specs omit
that extra input field. JSON specifications are losslessly gzip-compressed.

From the repository root, regenerate the two reports without engine access:

```sh
python3 tests/benchmarks/results/2026-11-22/perf-plan/watchlist-alias-fix/report.py \
  tests/benchmarks/results/2026-11-22/perf-plan/watchlist-alias-fix
```

The archived runner requires an explicitly owned native fixture configured as
in `tests/benchmarks/README.md`. To repeat timings, decompress the input specs
into a private directory and pass `--spec-dir`, `--out`, `--rows 1000000`,
`--indicators 1000`, `--repeats 5` and `--families shared,unrelated`; bound the
whole run to 900 seconds. Do not run builds or tests during measurements.
The reporter needs repository benchmark helpers and checks five repetitions,
warmup presence, complete-pair digests, summaries and successful cleanup.
`sha256.json` covers every payload except itself.
