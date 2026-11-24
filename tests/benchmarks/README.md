<!-- SPDX-License-Identifier: Apache-2.0 -->
# Performance measurements

Keep these routes separate when reading the results:

| Route | Measured work | Entry point |
|---|---|---|
| Native engine SQL over HTTP | Generated sweep predicates, source scans and ingest materialized views | `libwildcard/tests/benchmarks/sweeps.py --state …` |
| Direct `antalya-rs` | Published Rust client, HTTP transport, Arrow decoding and batch consumption | `tests/transport/examples/fdw_profile.rs` |
| PostgreSQL backend | PostgreSQL executor, `antalya-rs`, Arrow-to-datum conversion and local condition rechecks; EXPLAIN consumes results | `fdw_profile.py` |
| Full PostgreSQL delivery | Backend work plus binary COPY serialization, TCP delivery and complete result consumption through psql | `fdw_wire.py` |
| Elasticsearch | HTTP wildcard queries over separately indexed `wildcard` and `keyword` fields | `elasticsearch.py` |

The first route does not exercise `antalya-rs`. Direct-client and PostgreSQL
measurements use identical remote projections and options per paired workload.
The Elastic comparison uses the same ASCII corpus and verifies the same sorted
row/indicator/view pairs as batch sweeps. Its `_msearch` result contract and
indexing cost are reported separately from boolean-union searches and ingest
materialized views. Elastic JSON cannot represent arbitrary invalid UTF-8;
semantic probes are not a general RE2 equivalence claim.

Comparison targets must be declared `wildcard` fields or explicitly enabled
wildcard observables. Ordinary keyword, IP, numeric and hash fields are outside
this search scope. Hash lookup belongs to the typed equality path (a Bloom
filter can select candidates, followed by exact comparison). The optional
Elastic `keyword` layout is a storage control for the same eligible synthetic
values; it does not make keyword fields eligible in an application.
Typed equality, tenant and time predicates may still narrow the source rows;
they are filters alongside the wildcard search, not additional wildcard targets.

See [sweep reproduction](../../libwildcard/tests/benchmarks/README.md),
[FDW profiling](FDW.md), and [measurement summaries](results/2026-11-22/).
For the archived runs, settings, summaries, harnesses and provenance remain in
Git; their raw evidence is available in checksummed release assets.
[Verify archived evidence](ARTIFACTS.md) with
`python3 tools/verify_benchmark_artifacts.py` from the repository root.
Evidence for the current requalification has not been uploaded. Detailed
experiments and private engine state belong in `.local/`.

## Current requalification

The [single current performance table](../../README.md#measured-performance)
uses a fresh 2026-11-24 cohort for all eight routes. Canonical `libwildcard` has
an additive opt-in staged SQL planner available since 0.2.2. Measurements used
the qualified source before the version-only release bump. The PostgreSQL and
direct routes use `antalya-rs 0.1.1`, also shipped by extension release 0.2.2.

### Sweep protocol

- Shared and unrelated literal workloads each contain one million documents,
  1,000 wildcard rules and exactly 10,000 row/indicator/view pairs. All eight
  routes have one excluded warmup and seven measured repetitions in rotating
  order, with one client sweep at a time. The source and original query fields
  are unchanged from the frozen reference. Full source hashes, exact result
  multisets and all-indicator canaries are checked.
- Antalya 26.8.34.20001 and Elasticsearch 9.5.6 each have four CPUs and 8 GiB;
  Elasticsearch has a 4 GiB heap. PostgreSQL is 18.6 with pgrx 0.19.4. The ordinary
  native query permits four threads; parallel routes permit four simultaneous
  one-thread queries in disjoint 250-rule batches. `max_block_size=8192` is fixed.
  Arrow uses `flush_first` without compression. Elastic search concurrency is 1 or 4.
- Native/direct disable result and condition caches; Elasticsearch disables
  request/query caches. PG inherits fixture settings: each recorded PG query
  has two condition-cache misses and no hits. Text-index and filesystem caches
  remain uncontrolled. Four-CPU quotas do not reserve host CPUs.
- PostgreSQL uses the unchanged, previously qualified `current-011` FDW binary
  in a fresh PostgreSQL 18.6 container. Only its canonical handler is registered;
  these generated pair queries invoke no local `wc_*` function. This is a real
  sweep-route check, not a fresh qualification of the entire extension package.
  Direct execution likewise retains the qualified 0.1.1 worker. Binary hashes
  and the handler bootstrap are recorded in local provenance.
- The PostgreSQL EXPLAIN row reports the complete request with row timing off;
  binary COPY includes complete delivery and consumption. A full COPY precheck
  validates the exact SQL before backend timing. Every result-returning route
  checks the full pair multiset after its measured clock.
- Ingestion uses unequal default fsync acknowledgment durability. Its measurements
  do not establish a cross-engine ingestion ranking and are outside query clocks.

### Selective result and dense controls

The acceptance rule was fixed before timing: at least 10% lower median latency
than four indexed batches on each selective corpus, with at least six of seven
corresponding rounds faster. Both pass: **33.453% / 50.567% lower medians**, with
**6/7 / 7/7** faster rounds. These are local engineering criteria, not confidence
intervals or guarantees for arbitrary patterns and distributions. Staging also
has lower medians than the four-search Elastic route in these two fixtures;
this does not establish a general Elasticsearch ranking.

The maintained benchmark executor admits 10,000 candidates (1%) containing 290,000 / 340,000
raw-value bytes. Every staged sweep includes the count/byte preflight, CREATE,
population check, four exact queries and DROP: eight correlated QueryFinish
records. The preflight is new timed work relative to the exploratory prototype.
Median summed query CPU falls from **2,184 to 1,021 ms** for shared literals and from
**3,800 to 1,464 ms** for unrelated literals. Both staged sweeps read 2.05M logical
rows, including the two source scans, accounting and four candidate rechecks.
Shared indexed batches read only 1.32M rows, yet use more CPU: row count alone
misses the cost of repeated byte-index predicates and matching expressions.

Separate dense controls preserve rule results and value lengths while making
all one million source rows pass the cheap gate. Each has three repetitions
and a warmup for both native routes. Staging is refused every time before CREATE;
there are five requests: preflight plus four indexed batches. Reference → guarded
medians are **819.888 → 836.910 ms** and **1,463.906 → 1,452.278 ms**. The preflight
server clock alone is **56 ms [47–59] / 81 ms [78–93]**. The slight unrelated
median decrease is variability, not evidence that a refused staging plan saves
work; total query CPU rises in both controls. Refusal still incurs preflight cost.

### Paired FDW protocol

Six rounds cover all six orders of current FDW/client 0.1.0, predecessor
FDW/client 0.1.1 and current FDW/client 0.1.1. Each workload and route has one
excluded warmup per cohort. Workloads return one million rows except the wide
projection (32,768). Broad recheck preserves the local `wc_match` filter.
Settings are `in_band`, uncompressed Arrow, a 16 MiB batch budget,
`max_threads=2` and `max_block_size=1024`.

This separate **2026-11-23** study is retained as historical FDW evidence.
At fixed client 0.1.1, current FDW versus predecessor gives these median paired
latency reductions % [minimum, maximum]:

| Workload | PostgreSQL executor | Complete binary COPY |
|---|---:|---:|
| Narrow | 5.47 [-8.26, 27.26] | -0.68 [-32.21, 27.41] |
| Short text | -3.24 [-18.77, 4.49] | -4.96 [-15.55, 43.17] |
| Wide, 1 KiB | -15.50 [-33.54, 6.99] | -1.26 [-7.50, 5.84] |
| Broad recheck | 2.36 [-83.83, 15.44] | 3.98 [-25.96, 30.73] |

**No consistent FDW optimization gain is demonstrated:** every range crosses
zero and some workloads have slower medians.

The following comparison holds current FDW code fixed to measure the client
update. Values are **median round-paired reduction % [minimum, maximum]**.
Positive means lower latency; negative means slower. These are medians of
paired percentages, not ratios of pooled medians.

| Workload | Client 0.1.0 → 0.1.1: executor | Client 0.1.0 → 0.1.1: complete COPY |
|---|---:|---:|
| Narrow | 18.53 [1.09, 47.78] | 15.43 [-13.25, 30.76] |
| Short text | -0.33 [-16.41, 49.77] | 3.35 [-22.17, 51.20] |
| Wide, 1 KiB | 3.69 [-43.06, 35.99] | 8.46 [-0.26, 40.31] |
| Broad recheck | 7.85 [-73.79, 47.62] | 11.27 [-91.39, 42.83] |

The unchanged direct 0.1.1 worker in both optimization cohorts is a variability
control. Its changes cannot be attributed to FDW optimizations. Paired PostgreSQL
binaries use matched release profiles: fat LTO, one codegen unit and debug level 1.
The extracted extension package was built independently with its package defaults;
that build verifies packaging and was not substituted into the timing cohorts.

### Evidence and limits

These are local shared-machine observations. This task ran no builds or tests
during timing; unrelated host activity and filesystem caches were uncontrolled.
An independent recalculation verified counts, hashes and aggregates. The
measurements have not been independently reproduced in a controlled environment.
Overlapping clocks and route medians must not be subtracted to infer isolated
FDW overhead. Preparation counters are not exclusive CPU attribution.

Fresh evidence remains under `.local/staged-promotion/`: frozen specifications,
source/binary fingerprints, protocol and acceptance rule, full raw records,
correlated native profiles, verification scripts, qualification logs and cleanup
receipt. The final cohort contains 144 million-document sweeps including warmups
and dense controls, plus 16 small smoke sweeps. None is censored. The smoke exposed
an observer database-attribution bug, fixed and regression-tested before any
full-corpus timing; the interrupted setup supplied no timing sample. Temporary
fixtures, private state and superseded builds are removed after verification.

Historical `.local/final-matrix-20261123/` evidence covers the paired FDW study:
432 FDW records, 72 full-value controls and 84 earlier sweep records, including
warmups. Its verifier rejects seven intentionally corrupted cases. Neither local
campaign is included in the archived assets below; those downloads cannot verify
the current README table.

## Selective staging and bounded parallelism

The current generator emits `parallel_dispatch` and, for admitted literal
families, `staged_dispatch` from the canonical library API. With
`--layout byte_dispatch --concurrency 1`, explicitly select `native_parallel`
or `native_staged` in `--routes`. Neither is a default route. Ordinary `native`
runs one query with four engine threads; the parallel routes run up to four
250-rule queries with one engine thread each. They retain the same pair multiset.
This is application scheduling over Antalya, not threading inside the FDW.

`native_staged` times the complete lifecycle: candidate count and raw-value-byte
preflight, fresh Memory table creation, population verification, exact queries
and synchronous DROP. Its explicit admission policy is at most 100,000 rows,
64 MiB of raw values and 10% of the immutable source. A refused gate runs the
indexed parallel sweep before creating any table; its preflight cost still
counts. These thresholds are operator policy, not a proven optimal crossover.
The experiment qualifies 1% and 100% candidate populations, not every intermediate
selectivity. Do not truncate a candidate set to make it fit.

Four requests are admitted at a time, and completed replies are consumed before
admitting further work. Per-query memory is divided across workers; aggregate
pair limits, bounded HTTP replies and fixture container limits also apply.
Raw-value bytes do not measure RSS or all projected data. The library creates no
threads and owns no server state. This runner requires an exclusively owned,
immutable fixture database; a production executor must provide snapshot,
namespace, cancellation and cleanup guarantees itself.

The 60-second sweep limit is a sample censoring threshold. Each acquisition or
worker query receives the remaining execution budget, but HTTP transport allows
an additional 30 seconds and cancellation/join/cleanup can extend wall time.
Timed-out sweeps are not successful samples. Workers are cancelled and joined
before dropping their table; the parent retries owned cleanup on failure.

The native isolation regression requires an explicitly selected owned fixture:
`WILDCARD_BENCHMARK_STATE=.local/perf/antalya.json python3 -m unittest discover -s tests/benchmarks -p test_process_isolation.py`.
It checks process attribution from another database and cancellation isolation.
Use the `system.processes.current_database` column: `database` can resolve to
the observer's `database()` function and silently misattribute running queries.

Why this can help: the ordinary engine already uses multiple threads, but
combined query planning and rule work need not split evenly across them. Batching
rules exposes more independent work. Staging shares a smaller candidate population
across batches. This runner scans the source twice for a cheap necessary condition:
preflight and acquisition.
The Memory table has no text index, so the staged renderer omits the redundant
byte-index predicate while preserving exact rule checks. With a dense gate,
materialization and repeated candidate processing can outweigh any saving.

The preceding local exploration separates several mechanisms. A single native
query already had four read/transform branches in `EXPLAIN PIPELINE`; increasing
`max_threads` from one to four improved latency by 1.55× / 1.63× on shared /
unrelated rules. Running the **same four queries** serially or concurrently gave
2.49× / 2.56× improvements with unchanged logical row-read counts. These controls
demonstrate existing engine parallelism and a scheduling benefit; they are not
fresh samples in the README comparison.

Splitting rules also changes the work. The dispatch builder excludes trigram
postings with more than 64 members and greedily groups remaining rules. A fragment
that is too common in 1,000 rules can become useful in a 250-rule batch. Conversely,
each batch can revisit source granules. Shuffling the same source rows raised
shared-rule four-batch reads from 1.32M to 4M and removed an apparent benefit from
using eight batches. Four data partitions retained all 1,000 rules in each query
and replicated expensive expression preparation. These observations explain why
adding tasks or partitioning data is not automatically faster.

In the selective prototype, omitting the redundant index predicate on Memory
reduced summed query CPU from roughly 8–10 seconds to below one second. The
final prototype reduced CPU as well as elapsed time; it did not simply increase
core utilization. Deliberately admitting every source row reversed the gain:
unconditionally staged runs were 5.46× / 7.77× slower than indexed rule batches.
That counterexample motivated explicit admission and the timed fallback control.
The raw exploratory evidence remains in `.local/perf/parallel-sweeps-20261124/`
and `.local/perf/parallel-techniques-20261124/`; it is not a published artifact.

Summed CPU and logical reads support these explanations but do not assign every
millisecond to a function. Parse, analysis and plan counters overlap; they must
not be added as exclusive CPU costs. Query peaks are not concurrent process RSS.
The FDW currently registers ordinary scan callbacks, not parallel foreign-scan
worker callbacks. Remote engine parallelism is independent of that PostgreSQL
interface, and native orchestration gains do not establish a faster FDW route.

## Archived measurements

These runs retain their original versions, settings and conclusions. They are
historical context, not measurements of the current candidate.

- [Matched engine and route comparison](results/2026-11-22/perf-plan/byte-dispatch/):
  earlier million-document sweeps, including index/ingestion observations.
- [Antalya 0.1.1 codec experiment](results/2026-11-23/antalya-0.1.1/):
  Zstd reduced Arrow response bytes by 61% without a consistent latency benefit;
  PostgreSQL compression required an experimental patch.
- [FDW conversion and matcher experiment](results/2026-11-23/fdw-overhead/):
  earlier paired measurements and flamegraphs. Its broad-recheck gains are not
  confirmed as consistent gains by the current requalification.
- [FDW tuning and profiling](results/2026-11-22/perf-plan/fdw-tuning/):
  block sizes, terminal policies, selective lake completion and adaptive-cache churn.

## Owned fixtures

`fixtures.py` creates isolated Docker networks and containers from locally cached,
digest-pinned images. It never uses an existing service or pulls an image. Start
an engine with `python3 tests/benchmarks/fixtures.py start --engine antalya
--state .local/bench/antalya.json` (or `--engine elasticsearch`). The private state
contains credentials and must not be copied into results. Stop it with the same
command using `stop` and omitting `--engine`. Disconnect any profiling PostgreSQL
container from that owned network before stopping the fixture.
New fixtures keep configuration and shared files in an exclusively created
private directory recorded in their state. Existing parent permissions and
sibling files are preserved. Stop verifies resource labels and artifact ownership;
Docker inspection failures retain the state for a retry. Legacy states without
an artifact-directory binding leave sibling XML/shared paths untouched.

Run timed suites sequentially. Record resource limits, engine versions, cache
settings, exact result checks and failed/censored attempts. Engine and client
clocks overlap; adding them does not produce a stage breakdown. A retained batch
budget is not a process RSS cap. Shared development-host timings are descriptive,
not a universal speed guarantee.

CI runs 10,000 rows (1% of the full corpus) against all generated sweep and ingest
strategies, checks exact result pairs, adds an untimed positive canary for every
indicator across all sweep batches, and uploads timings without enforcing
performance thresholds.

Comparison fixtures also emit the production watchlist strategy in
`watchlist.join_plan`. `comparison_smoke.py --require-watchlist-plan` executes
that materialized view, verifies full hit multisets and every-indicator canaries,
and records informational insertion time with its selected strategy/fallback.
CI requires this field. Historical fixtures without it remain runnable without
the flag and explicitly report that watchlist qualification was not performed.

## Comparable workload matrix

`sweep_benchmark_gen --comparison shared|unrelated|common|short|mixed` emits a
versioned fixture and a Rust-matcher oracle. `compare_routes.py` loads identical
values into native and Elastic layouts and checks complete pair multisets. It
uses a warmup followed by rotating variant order, persistent direct-client and
PostgreSQL sessions, a start barrier for concurrent operations, and bounded
results/deadlines. Mixed-view RE2 fixtures explicitly exclude Elastic.

The additional `domains`, `urls`, `paths` and `commands` variants
exercise wildcard suffixes, prefixes, interior fragments and fixed-width `?`
patterns on their respective observable families. Each indicator has a positive
example and a realistic near miss; backgrounds retain the family's string
shape. These initial fixtures use case-sensitive raw ASCII values in both engines, with no implicit
normalization. Windows paths retain their literal backslashes; the wildcard
syntax escapes those backslashes. A normalized-path or reversed-domain layout
requires its own semantic and physical-index qualification.

Preserving raw paths does not mean sending their wildcard syntax unchanged to
RE2. The generator parses the canonical wildcard pattern into literal and
wildcard pieces, then compiles those pieces with `to_regex`. For example, the
following are actual values, before JSON or SQL string escaping:

```text
Raw path:          C:\Users\alice\Temp\drop.exe
Wildcard pattern:  C:\\Users\\*\\Temp\\*.exe
Compiled RE2:      (?s)\AC:\\Users\\.*\\Temp\\.*\.exe\z
```

Literal path separators and dots are escaped for RE2; `*` and `?` become regex
operators with the library's wildcard semantics. JSON and SQL encoding are
separate final steps. A UI accepting ordinary Windows path/glob notation must
translate that notation explicitly: the canonical wildcard API treats `\` as
an escape, so passing `C:\Users\*` directly would lose literal separators.
This syntax compilation is distinct from the optional `winpath` view, which
maps separators to `/` and folds ASCII case while retaining the original value.
The path fixtures check positive and near-miss values using the Rust wildcard
matcher, then check their compiled RE2 queries against ClickHouse.

These fixtures describe query semantics, not a claim that every family's
optimal physical layout is already implemented. Record the actual index and
query plan before attributing gains to an observable-specific index. The 1%
smoke checks full source-value hashes, exact match multisets and every-indicator
canaries for all nine variants.

The versioned `search_target` metadata declares a synthetic wildcard field for
generic distributions and commands, or explicit observable opt-in for domains,
URLs and paths. The runners reject missing or ineligible declarations before
creating engine fixtures. This is a benchmark admission check, not schema
discovery or a production routing implementation. A path-shaped value does not
make an existing keyword field eligible, and raw values are not retyped or
concatenated to bypass the policy. The library's low-level matchers remain
generic; callers must apply their schema and observable policy when choosing
search targets.

`compare_routes.py` defaults to Elastic's `wildcard` layout. Request the
`elastic_keyword` route explicitly when reproducing the historical storage
control, and label it separately from eligible application fields.

```sh
cargo run --release --locked --manifest-path libwildcard/Cargo.toml --features hot \
  --example sweep_benchmark_gen -- --comparison unrelated --indicators 1000 \
  --rows 1000000 > .local/comparison.json
cargo build --release --locked --manifest-path tests/transport/Cargo.toml \
  --features production --example sweep_route
python3 tests/benchmarks/compare_routes.py .local/comparison.json \
  --native-state .local/perf/antalya.json --elastic-state .local/perf/elasticsearch.json \
  --pg-container OWNED_PG --direct-worker tests/transport/target/release/examples/sweep_route \
  --layout byte_dispatch --ingest-durability defaults \
  --routes native,native_parallel,native_staged,direct,pg_backend,pg_copy,elastic_wildcard,elastic_wildcard_4 \
  --out .local/perf/comparison --repeats 7 --concurrency 1
```

PostgreSQL backend samples consume tuples with `TIMING OFF`; full COPY samples
consume the binary result through a persistent TCP psql session into a bounded
local file. COPY fields are decoded and compared after timing. Full COPY and
EXPLAIN clocks remain separate, and all-indicator canaries exercise batches
whose indicators are absent from a 1% source sample. Native query-log reads and
query memory are separate from client overlap, backend RSS and container limits.
Timeouts/resource failures are censored records, never successful samples.
If the PostgreSQL full-result precheck times out, both requested PostgreSQL
routes are censored before timing. The runner records `precontrol_failure`,
closes their sessions and confirms cancellation of owned native queries before
continuing other routes. Result mismatches and unexpected precheck errors remain
fatal. A censored precheck supplies no latency sample or correctness claim.

The index counterpart uses the existing materialized-column/text-index DDL.
Its actual EXPLAIN, source digest, index bytes and incremental insertion cost
are retained even when the generated predicates cannot use that index. New
experiments and intermediate results belong in `.local/`; publication is a
separate decision.

Client-sweep concurrency and engine query parallelism are separate controls.
Native queries permit four threads. Elastic uses one shard and
`--elastic-searches 1` by default; `--elastic-searches 4` permits four concurrent
searches inside each `_msearch` while keeping the same complete pair contract.
Record this setting when comparing engines: a single client does not imply a
single native execution thread, nor does `_msearch` imply one source query.

Native HTTP and the direct worker explicitly disable query-result and
query-condition caches. PostgreSQL inherits the native fixture's cache policy;
the FDW's supported settings do not include those overrides. Inspect the recorded
PG native query profiles when comparing routes. The historical byte-dispatch PG
profiles recorded condition-cache misses and no hits; this is different from
proving that all routes disabled the cache. Text-index and filesystem caches are
also uncontrolled.

For the bounded byte-fragment dispatch experiment, pass `--layout byte_dispatch`.
The generator includes this plan alongside the existing sweep plan. Unsupported
or fallback plans are rejected. Native HTTP, direct published Antalya and both
PostgreSQL routes use the same indexed source. The PostgreSQL routes consume the
generated exact-pair query through the FDW; they do not add local `wc_*` rechecks.
The default route set for this layout replaces `indexed` with
`elastic_wildcard_4`: two Elastic concurrency settings share one wildcard index.
`elastic_wildcard` uses `--elastic-searches` (default 1); the `_4` route always
uses 4. The legacy layout and its default routes are unchanged.

Use `fixtures.py start --engine elasticsearch --elastic-memory-gib 8 --state
.local/perf/elastic-fair.json` for the 8 GiB fixture with a 4 GiB heap, matching
the native fixture's 4 CPUs and 8 GiB container limit. The existing Elastic
fixture default remains 4 GiB with a 2 GiB heap. Select resources before timing
and record the effective limits; existing fixtures are not resized.

The byte layout uploads identical raw documents in 2,000-document HTTP batches,
with 90% initial and 10% incremental ingestion. Both indexes are maintained on
insert. Timers end at count-confirmed searchable visibility, including Elastic
refresh. Serialization, compaction and source-hash verification are separate.
`--ingest-durability fsync` (the byte-layout default) configures native
`fsync_after_insert=1,fsync_part_directory=1` and Elastic translog durability
`request`; effective settings are saved and checked. This aligns configured
fsync acknowledgment, without claiming hardware failure testing. The explicit
`defaults` mode preserves engine defaults, which have different acknowledgment
durability and must not support an ingestion speed ranking. Fixed batch sizes
and one build per case measure this workload, not maximum ingestion throughput.

Index use and selectivity are separate checks. The byte gate requires up to
three necessary fragments together for each rule, OR across rules, and retains
overlong values for exact rechecking. A broad OR of single common fragments can
use the index yet admit most rows. Inspect `EXPLAIN` actions for index-derived
`PREWHERE` filters and use forced-index checks to establish index use; measure
candidate counts separately. Logical `read_rows` and `read_bytes` alone do not
establish physical disk reads or whether the text index was used.

## FDW block size and terminal-policy controls

`fdw_tuning.py` uses an owned native fixture and the labeled PostgreSQL review
fixture to compare native HTTP Arrow, the persistent published Antalya client,
PostgreSQL `EXPLAIN (ANALYZE, VERBOSE, TIMING OFF)`, and full PostgreSQL COPY.

```sh
python3 tests/benchmarks/fdw_tuning.py \
  --state .local/perf/antalya.private.json --pg-container wildcard-review-pg18 \
  --worker .local/review-pg/target/release/examples/fdw_warm \
  --output .local/perf/block-controls \
  --blocks 1024,4096,16384 --policies in_band \
  --workloads narrow,smalltext,wide1k --repeats 5
```

Use a Python environment with PyArrow. The worker is built from
`tests/transport` with its `production` feature. The runner creates and removes
its own tables, schema and foreign servers. It excludes one full-value warmup
per configuration and rotates the order of five measured rounds. Narrow and
three-column cases contain one million rows; the wide case contains 32,768 rows
with a 1 KiB field. `--rows` and `--repeats` permit smaller functional smoke runs.

All routes explicitly request uncompressed Arrow. Actual QueryFinish records
must confirm row counts, block settings and compression; COPY IDs are bound to
the paired backend's selected SQL. Query-log diagnostics run after the entire
cohort. Requested `max_block_size` is not a guarantee of delivered batch size.
Full native/COPY values are checked outside their request clocks. Direct values
are checked inside excluded warmup clocks. Measured direct requests consume every batch and require a
confirmed successful outcome. Nested phase clocks overlap.

`--routes native` supports an independent HTTP control; requesting `pg_copy`
requires `pg_backend` for query correlation. Resource refusal or failed cleanup
makes the run fail and retains raw/pending evidence. Such a partial run is not
a complete performance cohort. Retained-batch budgets do not bound process RSS,
and host activity and engine caches must be considered when interpreting timings.

## FDW streaming, terminal and cache experiments

The ownership-checked `fdw_tuning.py` runner varies requested block sizes without
raising the 16 MiB output-batch budget. It distinguishes native HTTP, direct
published antalya-rs, PostgreSQL backend requests and complete binary COPY.
`fdw_terminal.py` adds a small-result control and eight genuine Parquet files,
imports a CLI-built Puffin index, and requires native counters proving one versus
eight files read. The Parquet control does not measure Iceberg metadata or deletes.
Both runners exclude a warmup and rotate measured configurations.

```sh
python3 tests/benchmarks/fdw_terminal.py \
  --state .local/perf/antalya.json --pg-container OWNED_PG \
  --worker tests/transport/target/release/examples/fdw_warm \
  --cli libwildcard/target/debug/wc-lake --repeats 5 \
  --expected-flush-ms 1000 --output .local/perf/terminal
python3 tests/benchmarks/fdw_cache.py \
  --state .local/perf/antalya.json --pg-container OWNED_PG \
  --rounds 5 --output .local/perf/cache
```

Use the Python environment with PyArrow for the terminal runner. The owned PG
fixture must mount this repository at `/workspace`; the psql client reads the
private CSV exports within a transaction. Output directories must be new. Record
the loaded query-log cadence: `--expected-flush-ms` checks the fixture configuration
but does not change or reload it. Change cadence and restart the owned native
fixture outside timing, then restore it. Read actual file counters, not only
`read_rows`, before attributing lake gains to pruning.

The cache runner exercises eight reused keys interleaved with forty new keys,
persistent versus fresh backends, settings identities and actual 30/61-second
insertion-expiry checks. Cold and warm phases are intentional; connection setup
is recorded separately from query time. Keep the same release-build flags when
comparing cache implementations. Never run builds or tests during timing.
