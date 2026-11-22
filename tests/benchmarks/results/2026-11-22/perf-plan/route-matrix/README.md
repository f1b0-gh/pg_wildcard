<!-- SPDX-License-Identifier: Apache-2.0 -->
# Comparable wildcard route matrix

This matrix preserves the full planned scope: shared, unrelated, common-gram,
short/no-gram and mixed-view fixtures at 100/1000 indicators over one million
rows; matched four-client controls for shared/unrelated/common-1000; the current
domain renderer at 100; and bounded 10,000-indicator attempts. Bounded refusals are
outcomes, not completed query timings. Semantic exclusions have no fabricated
Elastic comparison. See [the validated route table](matrix-report.md).
The [index tradeoff report](index-tradeoffs.md) separates query reads from
single-observation setup, insertion and settled-storage costs.
The [domain confirmation observations](domain-terminal-observation.json) preserve
individual direct/PG terminal diagnostics alongside native request clocks. They
support the existing terminal-confirmation follow-up without subtracting clocks
into exclusive PostgreSQL overhead.

Pattern diversity is reported separately from indicator count. In particular,
the short/no-gram fixture repeats the exact single-character pattern `q` under
different indicator IDs. It tests fallback and all-ID output, not a diverse short-
wildcard rule set. No broad engine conclusion should be drawn from that case.

Each uncensored route has an excluded warmup and five measured, interleaved
cohorts. Four concurrent clients yield twenty nested client samples, not twenty
independent repetitions. Native HTTP, direct published antalya-rs, PostgreSQL
backend execution and full TCP binary COPY have separate rows and clocks.
PostgreSQL backend execution uses TIMING OFF; full COPY consumption and the
exact pair checks before/after its clock are documented by the runner.
The matrix's foreign tables expose the generated native hit-pair queries. These
PG routes measure FDW transport/conversion, executor work and delivery of those
pairs; they do not add local wc_* rechecks or exercise the planner rewrite for
every original wildcard predicate. The separate broad-recheck FDW experiment
covers local rechecks and includes CPU profiles. Direct and PG sweep clients in
this matrix are configured with flush_first; the policy-control experiment
compares terminal strategies separately.

Elastic wildcard and keyword layouts receive the same raw corpus and supported
patterns, and return the same full row/indicator/view pair multiset. Keyword is
a historical storage-layout control; application wildcard-field admission still
excludes ordinary keyword/typed/hash fields. This is ordinary search, not an
Elastic percolator measurement. Engine/thread/search limits are explicit in each
case's settings and do not imply identical internal parallelism.

## Evidence

- Each completed directory contains raw samples/profiles, settings, exact SQL/DSL,
  corpus/pattern hashes, fixture specifications, native EXPLAIN and the original
  summaries. Every returned pair is validated; backend-only scans check executor
  rows after full-COPY precontrols on the identical SQL.
- matrix-storage.json retains native load/materialization/incremental-write and
  settled-storage observations, plus separate Elastic layout costs. Total storage
  is distinct from incremental index bytes. The separate byte-array index write
  experiment is a different private layout and is not substituted for these costs.
- Query-log rows/bytes, selected marks and query count describe different things.
  An index in EXPLAIN is not evidence of useful physical pruning. Filesystem caches
  and unrelated shared-host load were uncontrolled; no cold-cache or isolated-
  engine efficiency claim is made.
- Censored routes retain completed observations and the original refusal. Their
  partial medians do not establish a five-repetition speedup. Unsupported views
  and bounded fixture-generation/pair-budget refusals remain explicit.
- A configured deadline is not a uniform hard wall-clock cap across clients.
  Native sweeps check 60 seconds between queries and after completion; each HTTP
  request uses server max_execution_time=60 and a 90-second socket timeout.
  Elastic checks its sweep deadline between msearch batches and on completion.
  Direct sweeps use an outer Tokio timeout, and PostgreSQL a statement timeout.
  A completed request beyond the sweep deadline is still censored. The matrix
  dispatcher separately bounds each whole case to 2,700 seconds; exact settings
  and source snapshots preserve these different enforcement mechanisms.
- The PostgreSQL backend and COPY routes share persistent workers. If either
  timed route fails, cleanup closes those workers and disables both routes.
  A censored companion route may therefore have no independent failed attempt;
  the raw records identify the triggering route. Do not label an unattempted
  COPY route as an observed COPY timeout.
- The first common-gram 1,000-rule attempt stopped on PostgreSQL's 60-second
  full-COPY precheck timeout, before any timed cohort. Its unchanged raw setup
  records and cleanup proof remain under setup-failures. The collector now
  records that timeout as a PostgreSQL precheck refusal and continues the other
  routes after confirmed cancellation. Unexpected errors and oracle failures
  remain fatal. A fresh run uses the same fixture and limits; absent metadata
  from the original failed setup is not reconstructed.
- matrix-native-failures.json.gz retains post-run recovery of original native
  exception records scoped to each failing invocation. These include prechecks
  and warmups; they are not exclusive stage timings or a claimed one-to-one
  mapping to client failures. Cancellation entries can be consequences of cleanup
  after a client failure rather than its original cause. No query is replayed
  to obtain them. Local socket addresses in exception text are redacted in this
  public copy; export_provenance records that transformation and the SHA256 of
  the unchanged private recovery artifact.
- The measured direct sweep worker collapses its 60-second client timeout and
  execution errors into one generic failure marker. It does not retain a typed
  client phase/cause for that marker. Server exception recovery can add evidence,
  but cannot retroactively distinguish every client failure or establish its
  transport stage. Separate FDW batch-budget controls use a different worker and
  retain their typed resource refusals explicitly.
- Cases on either side of the host reboot remain distinct sessions. No interrupted
  C32 FDW result is used in this matrix; that separate attempt is preserved in the
  FDW control evidence.

## Reproduction and provenance limits

Restore completed case directories under .local/perf/matrix/, decompress their
gzip files, and place matrix-status.json under .local/perf/execution/. The supplied
report-matrix.py reads these records and the repository's comparison helpers to
validate summaries against raw samples and the exact fixture oracle without
contacting any engine. It regenerates the route and storage reports. Paths in
status records are repository-relative.

To repeat measurements, provision owned benchmark fixtures through the repository
benchmark instructions and use the supplied exact specifications with
tests/benchmarks/compare_routes.py. Use fresh output directories; never overwrite
historical measurements. runners/matrix.py records the original matrix's
generation, time and pair limits. Native and Elastic fixture credentials remain
private and are not part of this bundle.

settings.json records the measured collector/worker hashes. The additional helper
hash file is explicitly an export-time observation; it does not prove that every
historical helper had that content. Exact queries, fixtures and the original
per-run settings are preserved, but no full historical build attestation is
claimed. Some bounded generation outcomes have only the dispatcher's recorded
timeout budget/outcome; absent partial compiler output is not reconstructed.

SHA256 covers every payload, and compression is deterministic. Private fixture
state, endpoints, worker logs and credentials are excluded.
