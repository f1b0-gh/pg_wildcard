<!-- SPDX-License-Identifier: Apache-2.0 -->
# Performance plan evidence and decisions

Complete raw evidence and the original hash manifests are preserved in the
[checksummed release archive](../../../ARTIFACTS.md). From the repository root,
run `python3 tools/verify_benchmark_artifacts.py` to verify this historical run.


These experiments separate ordinary wildcard searches, incoming-event watchlists
and PostgreSQL delivery. They do not establish a single engine speed ranking.
Only explicitly admitted wildcard fields/observables enter these workloads;
ordinary keyword/typed/hash fields remain outside application wildcard admission.
An Elastic keyword layout appears only as a labeled historical storage control.

| Change or experiment | Decision | Evidence boundary |
|---|---|---|
| Preserve required interior grams in anchored patterns | Retained | Five paired native insertion trials in four wildcard families; exact output checks; unchanged command-line control |
| Raw-view watchlist rule-anchor join | Retained as opt-in; staged needles remain default | Prototype timing, production transfer and actual production update/block controls are separate experiments |
| Batch-scoped Arrow conversion preparation | Rejected and removed | Five paired release-binary rounds did not demonstrate consistent backend and full-delivery gains |
| Bounded raw byte-array index | Private experiment; no automatic production migration or dispatch API | Query observations and write/storage costs describe an explicit extra index layout |
| Regex engine replacement or specialization | No implementation selected | Existing match semantics and engines remain unchanged |
| Bounded hex-fragment index and raw-rule dispatch | Retained as opt-in; default sweeps unchanged | Required-fragment conjunctions, index-lifecycle qualification, equally resourced Elastic comparison and configured-fsync ingestion controls |

The [indexed dispatch follow-up](byte-dispatch/README.md) adds a later layout and
new measurements. It reduces unrelated-case candidates to 13,407 of one million
rows while retaining all 10,000 exact pairs. Elastic with four concurrent searches
still leads in the new comparable query runs. The historical matrix and rejected
experiments below remain separate; their numbers are not replaced by the follow-up.

See the [anchored-pattern evidence](anchored-watchlists/README.md),
[watchlist join evidence](watchlist-join/README.md),
[helper-alias correctness fix and transfer check](watchlist-alias-fix/README.md), and
[rejected FDW conversion experiment](fdw-conversion/README.md). The latter includes
eight real CPU flamegraphs. Failed/censored runs are preserved, not replaced by
smaller workloads or included as completed latency samples. The watchlist join's
per-value extraction bound does not bound global row-by-rule fanout or join memory.

The [FDW tuning follow-up](fdw-tuning/) adds repeated block-size controls,
small/lake terminal-policy and log-cadence experiments, an insertion-TTL-preserving
cache improvement, and a separately labelled opt-in upstream compression trial.

## Route and workload boundaries

Native HTTP SQL measures the server-facing query route. Direct antalya-rs uses
the published client and consumes every Arrow batch. PostgreSQL TIMING OFF
measures backend execution without full result delivery; complete TCP binary
COPY adds delivery to a client. Their wall clocks overlap with native/decode/
terminal diagnostics and cannot be subtracted into exclusive stage costs.
In the route matrix, foreign tables expose native-generated hit pairs; the PG
routes do not add local wc_* rechecks to each original wildcard predicate. The
separate broad-recheck FDW workload measures that work and has its own CPU
profiles. Matrix direct/PG clients use flush_first; those results must not be
silently substituted for the separate in_band conversion comparison.

The original process-startup measurements remain separate from later persistent-
connection measurements. Likewise, ordinary Elastic searches cannot stand in for
Elastic percolation. No equivalent Elastic percolator timing is claimed here.
The supported common semantic subset is checked using complete row/indicator/view
pairs; Unicode/invalid-byte controls remain explicitly native/Rust where cross-
engine equivalence has not been demonstrated.

Concurrency samples retain their hierarchy: clients within one cohort are not
independent repetitions. Backend RSS is not a transport batch budget, and sampled
peak RSS is not a proven absolute maximum. Database-referencing query counts can
include diagnostic requests and are labeled accordingly. First-batch arrival is
not network time to first byte.

## Completed 1,000-indicator search controls

These are separate one-million-row fixtures with the same complete hit-pair
contract on each admitted route, five measured repetitions after warmup. Each
contains 1,000 distinct regex/view pairs and returns 10,000 pairs. The table uses
complete-request medians; PostgreSQL executor times are separate in the full
route report. Source-query count is not Elastic search count or disk traffic.

| Fixture | Native HTTP s | Indexed native s | Direct antalya-rs s | PG backend request s | Full PG COPY s | Elastic wildcard s |
|---|---:|---:|---:|---:|---:|---:|
| Shared prefix | 2.631 | 2.828 | 4.783 | 4.664 | 4.581 | 2.373 |
| Unrelated rules | 20.313 | 21.046 | 20.772 | 20.757 | 20.502 | 1.880 |

The unrelated-rule case retains a substantial route-level gap to Elastic
wildcard search. Both native layouts execute two source queries and log two
million rows read in every measured sample; the added text index does not reduce
those reads. Meeting the two-scan budget therefore does not establish comparable
selectivity or performance. The shared-prefix result must not stand in for this
less favorable distribution. Native/indexed timing ranges overlap in both cases.

These are whole-route observations with explicit engine settings, different
storage layouts and uncontrolled shared-host load, not isolated regex-engine
benchmarks. The unrelated Elastic wildcard range is 1.806–7.667 s; native is
18.156–22.145 s. The full report retains every sample, all route ranges, setup
costs and storage. No Elastic percolator result is inferred from ordinary search.

The common-gram 1,000-rule case supplies no successful timing for native,
indexed native, direct antalya-rs, either PostgreSQL route, or Elastic wildcard.
PostgreSQL timed out during its full-COPY precheck; native HTTP streams were
incomplete, the direct route reported an execution failure, and Elastic wildcard
exceeded its sweep deadline. These failures remain explicit, with no speed ratio.
Only the historical keyword layout completed all five repetitions; it is not an
admitted application keyword-field workload or an Elastic wildcard substitute.
The original setup failure and the fresh run with corrected collector handling
are both retained. Post-run recovery preserves 32 original server exceptions across six failed
invocations, including server timeouts, cancellations and broken pipes. These
are invocation-scoped observations, not a one-to-one assignment of server causes
to client failures; cancellation and broken pipes can follow client cleanup.
The direct sweep worker combines its client timeout and execution errors in one
generic marker and does not preserve a typed client phase/cause for it. Those
failures cannot be assigned retrospectively to a transport stage; the separately
typed FDW batch-budget refusals are a different experiment.

The short/no-gram 1,000-ID control also completes all seven routes, but contains
only one distinct regex/view pair (`^q$`, raw). It tests fallback and preserving
all original IDs. Native and indexed native each execute one source query and
log one million rows read. Its performance must not stand in for a diverse set
of short wildcard expressions or an engine comparison.

The mixed-view 1,000-rule case completes five samples on each of its five admitted
routes, returning the same 10,000 pairs across 1,000 distinct regex/view pairs.
Native HTTP median is 28.962 s [24.976, 38.271]; indexed native 26.986 s
[24.380, 32.246]; direct antalya-rs 32.931 s [31.169, 41.109]; PostgreSQL backend
request 40.435 s [31.921, 41.009], with executor median 40.414 s; full PostgreSQL
COPY 36.732 s [33.090, 41.971]. Elastic is explicitly excluded for this mixed-view,
RE2-alternation fixture; there is no equivalent Elastic timing here.

Native issues seven queries while indexed native issues five. Every measured
query in both layouts reads one million rows and 123 marks, so the lower indexed
total of five million versus seven million logical reads comes from fewer
queries, not physical pruning by its text index. The indexed layout includes
materialized view columns and different generated SQL. Its total logical bytes
read are higher (774,675,040 versus 313,145,014 per sweep), and timing ranges
overlap. The two-source-query result for shared/unrelated raw rules is not a
universal limit for mixed views.

The shared-prefix four-client control completes five barrier-started cohorts per
route, with 20 client samples nested in those cohorts. Every route observes four
overlapping clients and four active engine searches, with no sampler errors.
Median client request times are native 7.619 s [6.394, 8.586], indexed native
7.324 s [6.102, 8.839], direct antalya-rs 7.938 s [6.868, 9.888], PostgreSQL
backend request 8.186 s [6.880, 10.225] (executor median 8.180 s), full COPY
8.021 s [6.585, 9.818], and Elastic wildcard 1.717 s [1.569, 1.983]. The
historical keyword layout is separately reported at 25.957 s [24.872, 29.121].
All return the same 10,000 pairs per client. Native routes log eight source
queries/eight million logical rows per cohort: still two queries/two million
rows per client, with no indexed-layout read reduction. Median complete-cohort
throughput is 0.482 native sweeps/s versus 2.256 Elastic wildcard sweeps/s;
these include the runner's protocol/result-processing boundary, not exclusive
engine time. This control preserves a substantial route-level concurrency gap;
the earlier C1 shared-prefix result must not imply parity under load. C1 and C4
are separate sessions on a shared host, not paired code-change measurements.

The unrelated-rule four-client case completes with censored native/PG outcomes.
All native HTTP and indexed-native warmup clients fail: three in each layout
finish after the 60-second sweep deadline and one returns an incomplete stream.
All direct clients return the generic timeout/execution-failure marker. All four
PostgreSQL backend clients hit the statement timeout; closing their shared
workers disables COPY before it is attempted. These are zero successful measured
samples for those routes, not timings or an independently observed COPY timeout.
Every failed cohort confirms cleanup before the next route starts.

Elastic wildcard completes five cohorts/20 client samples on that same fixture,
median 1.970 s [1.721, 3.288], with four observed clients and engine searches.
The historical keyword layout also completes, at 35.429 s [34.189, 41.017]. Both
validate all 10,000 pairs per client, with no sampler errors. This is a material
bounded-concurrency limitation of the current native routes; no speed ratio is
computed using censored requests, and no native read count is inferred from
the single-client case. Original client failures remain in the raw evidence.

The common-gram four-client case also retains its unfavorable outcomes. The
PostgreSQL full-COPY precheck times out before measurements, native/indexed
warmups return incomplete streams for all clients, direct returns its generic
failure marker, and every Elastic wildcard warmup client exceeds the sweep
deadline. Each cleanup is confirmed. Only the historical keyword layout completes
five cohorts/20 clients, median 32.384 s [30.532, 48.776], with exact pairs checked.
There is no successful native, direct, PostgreSQL or Elastic wildcard timing for
this case and no speed ratio; the keyword result is not substituted for them.

## Current domain renderer

This separate one-million-row case uses 100 distinct domain patterns and returns
5,000 exact pairs. All seven routes complete five measured samples after warmup.

| Route | Request ms median [range] |
|---|---:|
| Native HTTP | 584.211 [554.335, 608.178] |
| Indexed native | 639.064 [559.858, 939.534] |
| Direct antalya-rs | 644.490 [590.726, 1661.495] |
| PostgreSQL backend request | 1706.948 [617.424, 1727.526] |
| Full PostgreSQL COPY | 1503.466 [577.952, 1603.273] |
| Elastic wildcard | 355.926 [315.719, 426.060] |
| Elastic keyword, historical layout | 2707.365 [2497.719, 2756.205] |

Native and indexed native each issue one query and read one million logical
rows; the indexed layout does not reduce those reads. PostgreSQL executor median
is 1704.266 ms. Its four slower samples record terminal confirmation of
1069.321–1086.912 ms and 13 data control requests, versus 29.185 ms/two requests
in its faster sample. Direct also has one 1121.587 ms terminal observation;
its other four are 33.391–43.855 ms. These recorded diagnostics support the
existing [terminal-confirmation follow-up](https://github.com/f1b0-gh/pg_wildcard/issues/19).
They do not isolate exclusive stage costs or establish that conversion accounts
for the difference between route medians. Individual observations and raw-record
hashes are retained in the route-matrix evidence.

## 10,000-indicator shared-prefix stress

This one-million-row fixture has 10,000 distinct regex/view pairs and 10,000
expected output pairs. Indexed native completes five measured samples, median
55.506 s [34.150, 58.756]; Elastic wildcard completes five, median 16.223 s
[13.990, 29.019]. Native HTTP completes four samples, partial median 45.774 s
[36.570, 57.411], then exceeds the sweep deadline. Direct completes three,
partial median 47.786 s [45.695, 48.442], then returns its generic failure marker.
Those partial observations do not establish five-repetition speed ratios.

PostgreSQL backend has one successful measured request at 51.909 s (executor
51.856 s). The first measured COPY request then hits the statement timeout;
cleanup closes the shared workers and stops both PG routes. The historical
keyword layout fails its warmup deadline. Warmup success therefore does not
establish reliable completion across repeated large sweeps. Every failed cohort
confirms cleanup, and all successful result samples pass their exact-pair checks.

Each successful native-family sample logs 18 source queries and 18 million
logical rows, including indexed native. The index does not reduce those reads.
The generated-query count is not bounded to two at 10,000 indicators. Different
completion counts and overlapping ranges do not establish an index benefit;
shared-host load and the original failures remain in the evidence.

## 10,000-indicator unrelated-rule stress

This one-million-row case has 10,000 distinct regex/view pairs and returns
10,000 exact pairs. Elastic wildcard completes all five measured repetitions,
median 13.466 s [12.976, 13.905]. Every other route has zero successful measured
samples: both native layouts exceed the sweep deadline during warmup; direct
returns its generic failure marker; PostgreSQL times out in the full-COPY
precheck; and the historical keyword layout exceeds its warmup deadline.
All failure cleanup is confirmed. There is no native/Elastic speed ratio.

The generator emits 18 queries per native layout, but no complete native sweep
is measured here. This count does not establish 18M actual rows read for failed
requests. The successful Elastic samples validate the full output, not merely
its cardinality. This result preserves a material scale limitation of the
current native routes on unrelated patterns, separately from the shared-prefix
stress case and without attributing it to a particular regex engine.

## Bounded stress outcomes

The full matrix has 19 terminal case outcomes: 16 completed runners (including
censored routes), two fixture-generation timeouts, and one oracle-budget refusal.
Common and mixed 10,000-rule fixture generation exceeded the predefined 60-second
limit. The short 10,000-ID fixture repeats one pattern and exceeds the 100,000-pair
oracle limit. These cases have no query timing or throughput claim. Limits were
not raised after observing the failures.

## PostgreSQL resource and policy controls

The [FDW control bundle](fdw-controls/README.md) preserves C1/C8/C32 observations,
all four terminal policies, and direct/PG checks at 1 MiB and 4 MiB batch allowances.
Both direct and PostgreSQL refuse the small-budget wide-row controls with typed
resource-limit errors before timed samples, with confirmed cleanup. Successful
individual batch charges respect their allowance; this does not bound backend
RSS or aggregate retained allocations. C8/C32 each have one measured cohort after
warmup, not five independent repetitions. The reboot-interrupted C32 attempt is
preserved separately; the successful replacement is explicitly identified.

The [byte-index write-cost bundle](native-index-write-cost/README.md) retains five
paired trials. Indexed incremental insertion costs 6.40 times the plain layout and
adds 6,170,663 compressed index bytes on this fixture. This is a private index
layout, not an automatic production migration. Its original runner omitted a
start-time machine/settings snapshot; original INSERT profiles were recovered
without replay. Post-run observations do not retroactively establish settings.

## Reproducible evidence

The [complete route matrix](route-matrix/README.md) includes exact queries and
DSL, raw samples, settings, unsupported/censored outcomes and a separate
[index tradeoff report](route-matrix/index-tradeoffs.md). Payload manifests verify
all files. Local socket addresses are redacted in recovered exception text;
the export records that transformation and the original private artifact hash.

Ten route/index/domain/FDW reports regenerate byte-for-byte from the sanitized
bundles and public repository helpers, without private fixture state or engine
access. See [the reproduction receipt](../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-22/perf-plan/reproduction/receipt.json`). From the
repository root, run:

```sh
python3 tools/verify_benchmark_artifacts.py
```

The helper verifies manifests and regenerates reports in a temporary directory
under .local; socket connections and subprocess creation are blocked inside the
reporters. No benchmark queries are replayed. The per-bundle README files explain
how to repeat measurements separately with fresh owned fixtures.
