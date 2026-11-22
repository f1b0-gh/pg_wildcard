<!-- SPDX-License-Identifier: Apache-2.0 -->
# Sweep scaling experiment

From the repository root, with the qualified Python test environment:

```sh
mkdir -p .local/sweep-benchmark
cargo run --locked --manifest-path libwildcard/Cargo.toml --features hot \
  --example sweep_benchmark_gen > .local/sweep-benchmark/queries.json
for strategy in original_hyperscan constant_candidates exhaustive_exact; do
  timeout 300 python3 libwildcard/tests/benchmarks/sweeps.py --strategy "$strategy" \
    < .local/sweep-benchmark/queries.json \
    > ".local/sweep-benchmark/$strategy.log" 2>&1
  echo "$?" > ".local/sweep-benchmark/$strategy.exit"
done
```

This uses one million synthetic URL rows, one thousand ASCII indicators and
10,000 expected hits. Each strategy gets up to three timed repetitions. A completed sweep sample
over one minute suppresses further repetitions of that strategy. A process
that reaches the five-minute timeout does not start another repetition. Exit
124 means the remaining measurement is censored, not a completed query time.
Generated sweeps include all their batches in the reported time. Setup is
untimed, and every completed measurement must return the expected hit count.

The harness records engine version, query bytes, batch count, memory and AST
limits, block size, thread count, and short-circuit settings. Separate probes
compare `AND` and `if` under disabled, enabled and forced short-circuiting. Those
probes do not count regex evaluations or prove a regex compiles only once;
constant regex arguments make engine optimizations possible, with execution
behavior depending on the engine and settings.

The old Hyperscan strategy is included only as an ASCII performance baseline.
It is not a correctness gate: Unicode and invalid UTF-8 can disagree with RE2.
The differential suite covers those inputs separately. This synthetic corpus
and a shared development machine cannot establish a general speed guarantee;
retain the settings, all samples, and any concurrent-load caveat with results.

For dynamic watchlists, use the same generated specification and run each
strategy with `--ingest --repeats 1`; the three strategy names are
`original_hyperscan`, `exhaustive_exact`, and `dynamic_needles`. This creates an
actual watch table and insert-triggered MV, times `INSERT INTO ingest_src SELECT
...` separately from setup, and verifies the hits table against the known
10,000-hit corpus oracle. The metadata is populated by generated validated
inserts. Input and insertion blocks are bounded to 8,192 rows, including the
minimum insertion block settings, to keep the exhaustive cross-product within
the recorded memory limit. Throughput is sensitive to these settings.
