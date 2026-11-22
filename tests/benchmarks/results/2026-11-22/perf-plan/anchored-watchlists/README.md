<!-- SPDX-License-Identifier: Apache-2.0 -->
# Anchored watchlist insertion measurements

An actual ClickHouse materialized view checks 100 indicators on one million
synthetic rows. Only required-substring metadata changes between variants;
source values, regexes, view bindings and materialized-view SQL are identical.
The correction retains required interior trigrams when a pure conjunction also
contains start/end boundary grams. Exact regex checks remain in place.

These clocks measure native HTTP insertion through completion of the materialized
view. They do not include the antalya-rs client or PostgreSQL delivery. One warmup
per strategy precedes five interleaved measured repetitions. Complete
row/indicator/view multisets are checked after each timer, and positive/near-miss
canaries precede timing. Only completed five-pair families are included below.
No Elastic percolator timing is included. Ordinary Elastic searches over already
indexed documents perform a different operation and are not compared here.
The command-line variants have identical metadata and SQL, verified in
[input-comparison.json](input-comparison.json); their timing variation is an
unchanged-input control, not a claimed optimization benefit.

| Family | Strategy | Measured n | Insertion seconds: median [range] | Exact pairs |
|---|---|---:|---:|---:|
| domains | baseline | 5 | 258.721 [246.205–270.921] | 5000 |
| domains | interior_grams | 5 | 15.235 [13.114–16.382] | 5000 |
| paths | baseline | 5 | 158.663 [154.265–162.630] | 5000 |
| paths | interior_grams | 5 | 14.247 [13.660–14.313] | 5000 |
| urls | baseline | 5 | 32.667 [23.241–37.282] | 5000 |
| urls | interior_grams | 5 | 17.933 [14.325–21.783] | 5000 |
| commands | baseline | 5 | 13.666 [12.918–15.537] | 5000 |
| commands | interior_grams | 5 | 13.672 [12.909–14.155] | 5000 |

Each family contains raw query-log profiles and HTTP clocks (`raw.jsonl`), engine
and machine settings, the complete baseline/new specifications (`specs.json.gz`),
summary statistics and full-scale candidate counts. Query IDs, corpus hashes,
pair hashes and host load are preserved. ProfileEvents include CPU accounting
and regex-cache activity; local-cache lookups are not exclusive regex time.
Candidate counts denote substring-precheck passes, not an instrumented count of
regex invocations. Query peak memory is not backend RSS or a batch-budget bound.

The engine is pinned by image digest in [provenance.json](provenance.json), with
4 CPUs and 8 GiB. The host is shared, and filesystem caches are uncontrolled;
these are warm observations, not cold-cache results. The first domain baseline
warmup exceeded its 60-second budget: [censored-60s.json](censored-60s.json).
The fresh comparison used the existing 300-second ingestion budget. That failed
attempt contributes no completed timing sample and is not hidden in the median.

## Reproduction

[runner.py](runner.py) is the exact experiment snapshot, identified by SHA-256
alongside its public helper files in provenance.json. Run it from the repository
root with the same helper versions. It expects a private owned engine fixture
at `.local/perf/antalya.json`, created with `tests/benchmarks/fixtures.py`.
For each family, decompress `specs.json.gz` and write its `baseline` object to
`.local/perf/scoped-specs/<family>-100-1000000.json` and its `interior_grams`
object to `.local/perf/anchored-needles/specs/<family>-100-1000000.json`.
Then invoke the runner with `--timeout-seconds 300 --out <fresh-private-directory>`.
The complete run requires domains, paths, urls and commands specifications;
an incomplete result bundle is not sufficient to replay all four families.
Keep benchmark runs sequential and do not compile concurrently with timing.

The Windows fixture preserves literal path separators. Canonical wildcard
patterns are compiled to RE2 separately from SQL/JSON escaping; these measurements
do not implement a UI adapter for ordinary Windows glob notation. Eligible
targets are declared wildcard fields or explicitly enabled wildcard observables.
Typed keyword/hash searches are outside this fixture's scope.
