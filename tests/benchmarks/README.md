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
