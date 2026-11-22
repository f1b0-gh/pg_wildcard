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
[FDW profiling](FDW.md), and [committed measurements](results/2026-11-22/).
Detailed experiments and private engine state belong in `.local/`; only the
requested sanitized raw measurements, settings and their summaries are committed.

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
  --out .local/perf/comparison --repeats 5 --concurrency 1
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
Full native/COPY values and direct warmup values are checked outside their
request clocks. Measured direct requests consume every batch and require a
confirmed successful outcome. Nested phase clocks overlap.

`--routes native` supports an independent HTTP control; requesting `pg_copy`
requires `pg_backend` for query correlation. Resource refusal or failed cleanup
makes the run fail and retains raw/pending evidence. Such a partial run is not
a complete performance cohort. Retained-batch budgets do not bound process RSS,
and host activity and engine caches must be considered when interpreting timings.
