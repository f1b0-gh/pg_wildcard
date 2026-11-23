<!-- SPDX-License-Identifier: Apache-2.0 -->
# FDW concurrency, terminal policies and batch limits

Complete raw evidence and the original hash manifests are preserved in the
[checksummed release archive](../../../../ARTIFACTS.md). From the repository root,
run `python3 tools/verify_benchmark_artifacts.py` to verify this historical run.


These are baseline-library controls following the rejected conversion experiment.
No regex engine, terminal policy default or batch setting was changed by them.
Native source values and returned projections are identical across the three
routes: direct published antalya-rs, PostgreSQL backend execution with TIMING OFF,
and complete binary COPY over TCP. The reports keep those clocks separate.

- [Concurrency observations](concurrency-report.md): C1 has five separate baseline
  rounds. C8/C32 each have one measured barrier-started cohort after warmup. Their
  clients are nested samples, not independent repetitions. Narrow means one
  million integers per client; wide means 32,768 rows with a 1 KiB payload per client.
- [Policy and budget controls](controls-report.md): three measured samples after
  warmup for each completed route. Four terminal policies use the narrow fixture;
  smaller batch allowances use wide rows and in_band. Explicit resource refusals
  are preserved separately from successful latency observations.

Complete direct values were checked before measured scans; each measured direct
scan consumed all batches and checked the row count. Every measured COPY result
was checked field by field, including its complete unique row-ID set, after its
clock stopped. Backend scans checked Actual Rows. Successful runners cleaned up
their owned schemas/databases; cleanup.json records those outcomes.

## Interpretation

The server sampler counts queries whose text references this invocation's
database. It excludes itself but can include diagnostics. Its peak is therefore
not an exact source-scan concurrency count; the C32 wide backend case records 33
such queries with 32 overlapping clients. The raw observation is retained.

Backend RSS/threads are sampled separately from individual BudgetedBatch charge.
Summed backend RSS can count shared pages repeatedly. Neither RSS nor an
unmeasured aggregate retained-allocation peak is bounded by one batch allowance.
First batch is not network TTFB. Decode, polling, terminal and backend clocks can
overlap; subtracting route medians does not produce exclusive stage timings.

C1/C8 ran before a host reboot; C32 was repeated afterwards. Shared-host load and
filesystem caches were uncontrolled. These observations are not a controlled
single-session scaling curve. The original C32 attempt is preserved under
interrupted-c32 with eleven complete cohorts, trailing damaged bytes and empty
summary/cleanup files. It is excluded from the successful table. interruption.json
records its raw hash and the later cleanup of its owned resources; it does not
invent a successful original exit status. No timeout alone triggered a restart.

## Provenance and reproduction

Each completed run retains its exact SQL, native query IDs/profiles, worker setup,
machine/settings observations, host load, result checks and sampled resources in
raw.jsonl.gz and settings.json. The recorded runner hash is checked against the
included fdw-benchmark.py snapshot. restoration.json identifies the measured
baseline extension verified before the restarted queue; binary/build provenance
also appears in the separate FDW conversion evidence bundle. No full historical
build attestation is claimed.

To regenerate reports without an engine, restore run directories under
.local/perf/fdw/, decompress their raw.jsonl.gz files, restore the interrupted
directory as concurrency-32-interrupted-reboot, and copy control-status.json to
.local/perf/execution/remaining-status.json. Place report scripts from runners/
in that execution directory; also copy pg-budget-status.json there. Place reports
from this bundle under .local/perf/fdw/ and run report-fdw-concurrency.py followed by
report-fdw-controls.py. The resource-refusal classifier is already recorded in
control-status.json. Each refused run includes resource-refusal.json containing
the exact typed error extracted from original private stderr and its source-log
hash. The reporter can use this sanitized proof when the private log is absent;
no private diagnostic log or credential is required to regenerate the report.

The measurement runner requires the repository's existing persistent client and
PostgreSQL benchmark helpers plus an owned fixture. Never point it at production.
Use fresh output directories to repeat trials. SHA256 covers all payloads;
compression is deterministic. Credentials, endpoints and private worker logs
are excluded. No raw stack-memory recording is part of this bundle.
