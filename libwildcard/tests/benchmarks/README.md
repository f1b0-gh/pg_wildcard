<!-- SPDX-License-Identifier: Apache-2.0 -->
# Sweep scaling experiment

The full fixture contains one million synthetic URL rows, one thousand ASCII
indicators and 10,000 matching row/indicator/view pairs. Timed batch sweeps return
those pairs. Ingest measurements time insertion into an actual source table with
a materialized view; reading and checking its hit table happens after the timer.

From the repository root:

```sh
mkdir -p .local/sweep-benchmark
cargo run --locked --manifest-path libwildcard/Cargo.toml --features hot \
  --example sweep_benchmark_gen > .local/sweep-benchmark/spec.json
python3 tests/benchmarks/fixtures.py start --engine antalya \
  --state .local/sweep-benchmark/antalya.json
python3 tests/benchmarks/run_sweeps.py .local/sweep-benchmark/spec.json \
  --state .local/sweep-benchmark/antalya.json --out .local/sweep-benchmark/results
python3 tests/benchmarks/fixtures.py stop --state .local/sweep-benchmark/antalya.json
```

Native HTTP measurements exercise engine SQL, **not direct `antalya-rs`**.
For embedded chDB semantic checks, omit `--state` when invoking `sweeps.py`:

```sh
python3 libwildcard/tests/benchmarks/sweeps.py --mode sweeps \
  --strategy constant_candidates < .local/sweep-benchmark/spec.json
python3 tests/benchmarks/smoke.py .local/sweep-benchmark/spec.json \
  --out .local/sweep-benchmark/smoke
```

Strategies include historical ASCII Hyperscan, exhaustive exact checks, the
previous fixed-64 constant-candidate batches, and byte/AST-budgeted batches.
Ingest compares Hyperscan, exhaustive matching, the old lexical single needle,
three heuristically ranked needles, and optionally file-frequency-ranked needles.
The Hyperscan baseline is not a correctness gate outside this ASCII fixture.
Unicode, unsupported plans and invalid UTF-8 are covered by differential tests.

For real file-frequency measurements, generate a Parquet/Puffin fixture with
`tests/benchmarks/frequency_fixture.py --help`, then pass its exported
`gram-frequencies.csv` to `sweep_benchmark_gen --frequencies PATH`. Frequencies
change ranking only; every selected needle must still be required by the plan.
`--mode candidates` reports the fraction of row × indicator pairs passing the
substring predicate, not a directly instrumented regex invocation count.

Each variant gets up to three repetitions; a completed sample lasting at least
60 seconds suppresses further repetitions. Native queries have a 300-second
server limit. The suite preserves failed and censored runs and cancels only its
own query IDs before proceeding. Timeouts are not completed timings or medians.
All samples, engine query IDs, `read_rows`, profile events, query bytes and
settings are retained. The 1,000-indicator fixture must use at most two source
queries and read at most twice the source row count on the native engine.
Larger view expressions can legitimately require more batches.

Setup and correctness checks are untimed. Report median corpus rows per second
rather than summing source rows read across repeated scans. Query and condition
caches are disabled; filesystem caches are uncontrolled. Short-circuit behavior
depends on the engine/settings; constant regex arguments do not by themselves
prove compilation or invocation counts. Use the same engine for comparisons and
retain machine, load and timeout caveats alongside results.
