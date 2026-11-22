<!-- SPDX-License-Identifier: Apache-2.0 -->
# Opt-in native watchlist candidate join

The production `watchlist_plan` / `wc_ch_watchlist_plan` API can select an explicit
raw-view rule-anchor join. It finds candidate rules from required byte trigrams
and retains the existing exact `match()` check. Staged needles remain the default
and the fallback for other configured view sets. Bound metadata, duplicate source
rows and distinct indicator IDs are preserved. No regex engine changed.

All timings below are native resident-source INSERT SELECT operations including
source and hit-table persistence. These are percolation measurements, not
searches over already indexed documents. There is no comparable Elastic
percolator, antalya-rs or PostgreSQL timing in this bundle. The host is shared;
filesystem caches are uncontrolled. Warmups are excluded from medians and full
hit-pair checks happen outside timers.

## Original private prototype, one million rows

Five interleaved pairs per case. These numbers measure the original prototype,
not the later production renderer. Exact SQL and all raw profiles are included.

| Family | Rules | Staged seconds median [range] | Prototype join seconds median [range] |
|---|---:|---:|---:|
| domains | 100 | 12.902 [12.799–13.264] | 6.313 [5.954–6.585] |
| paths | 100 | 14.047 [13.509–14.206] | 6.648 [6.508–6.871] |
| urls | 100 | 13.724 [13.572–13.962] | 6.568 [6.329–6.975] |
| commands | 100 | 13.216 [12.739–13.610] | 6.144 [5.989–6.675] |
| shared | 1000 | 123.425 [106.701–217.260] | 9.432 [7.643–13.620] |
| unrelated | 1000 | 101.110 [99.107–125.662] | 8.351 [7.816–14.748] |

## Separate production-renderer confirmation

Each row is a separate five-pair session, with identical corpus, rule definitions
and exact outputs within the comparison. Do not combine the following timings
with the preceding table to calculate a transitive production speedup.

| Session / family | Prototype seconds median [range] | Production seconds median [range] |
|---|---:|---:|
| production-confirmation-100/domains | 5.735 [4.218–5.891] | 5.613 [4.649–5.995] |
| production-confirmation-1000/shared | 6.977 [6.123–7.246] | 6.689 [6.321–7.227] |
| production-confirmation-1000/unrelated | 5.964 [5.415–7.170] | 6.670 [5.575–7.354] |
| production-confirmation-unrelated-reverse/unrelated | 5.011 [4.616–5.582] | 4.885 [4.610–5.253] |

The original unrelated case regressed by 11.85% in its ratio of medians. It is
retained, not replaced. A reversed-starting-order repetition was added; all ten
pairs yield prototype/production medians of 5.499/5.414 seconds (-1.54%), while
the median paired change is +1.744%. See the [balanced report](production-balanced.md).
This addresses the numerical investigation threshold without proving that every
earlier difference came from noise.

## Actual production updates and block controls

The [update report](updates/report.md) compares production with staged needles
directly: 10,000 rows, 100 or 1,000 rules, requested block sizes 1,024 and 8,192,
five interleaved pairs each. Existing rule definitions rotate among stable IDs;
the complete shifted-ID oracle is checked after every update. Physical deletion
and reinsertion also pass. The update and subsequent insertion clocks are
separate. No new-pattern compilation or concurrent-update isolation claim is made.
Requested block settings do not establish the actual internal block distribution.

## Unfavorable cases and correctness

The [prototype fallback report](prototype-fallback-controls/report.md) preserves
the no-needle timeout and long-value memory refusal. The former has only three
paired completions; the latter has no successful baseline timing. Neither is a
five-pair performance gate or a speedup ratio. Common-gram and stale-binding
prototype controls have five pairs. These do not replace production semantic
qualification, which runs the actual generated materialized view.

Native and chDB seeds 1, 42 and 1234 all pass. The actual-MV corpus checks full
multisets through updates, deactivation, empty watchlists and stale bindings,
including duplicate rows, duplicate patterns with distinct IDs, qualified/empty
projections, internal-name collisions, materialized raw mappings, Unicode,
invalid bytes, NUL and values around/beyond the extraction cap. Raw qualification
logs and exit records are included. Only declared wildcard fields or explicitly
admitted wildcard observables enter the application fixtures; ordinary keyword
and hash fields are excluded.

The 256-byte extraction cap bounds keys per source row, not total join fanout or
query memory. Unsupported/absent needles and long values retain exact fallback;
unselective workloads can still be expensive. Bindings detect stale metadata,
not forged metadata: direct rule writers must supply proven required needles or
leave them empty. Existing MVs are not migrated by IF NOT EXISTS DDL.

## Reproduction and provenance

Each experiment includes its exact runner snapshot, SQL/fixture specifications,
settings, query profiles and hashes. Specifications and raw records are gzip
compressed. Absolute worktree prefixes in the fallback source-hash map are
normalized to repository-relative paths; the source hashes are unchanged.
Restore the corresponding private paths expected by the runner and
use an owned native fixture from the [benchmark setup](../../../../README.md); fixture credentials
are deliberately absent. Run timings sequentially, without concurrent builds.
Production and prototype scripts have separate snapshots; do not substitute the
current renderer for a historical measured query. The update runner expects the
production specifications under `.local/perf/percolator/production-specs/`.
All samples and the source hash were checked before export.

For example, decompress the four `prototype-full-100/<family>/spec.json.gz`
files as `.local/replay-specs/<family>-100-1000000.json`, then run from the
repository root (with the owned fixture at `.local/perf/antalya.json`):

```sh
python3 tests/benchmarks/results/2026-11-22/perf-plan/watchlist-join/prototype-full-100/benchmark.py \
  --indicators 100 --families domains,paths,urls,commands --rows 1000000 --repeats 5 \
  --block-rows 8192 --spec-dir .local/replay-specs --out .local/replay-watchlist-100
```

The 1,000-rule run uses its own runner/specifications and `--families shared,unrelated`.
Production confirmation uses the respective `production-confirmation.py` snapshot
with the same arguments; add `--reverse-order` only for the reversed session's
runner. Reproducing updates uses `updates/runner.py` and a fresh checkout/private
directory: its fixed `.local/perf/percolator/update-cost` output must not exist.

The [decision](decision.json) retains the explicit raw-view option and leaves the
default unchanged. The completed whole-stack review and its five fixes are
recorded in [review resolution](../review-resolution.md). The source alias fix has
a separate [qualification and transfer check](../watchlist-alias-fix/README.md);
these historical timings continue to describe their original source hash.
No universal latency improvement or regex-engine ranking is asserted.
