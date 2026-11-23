<!-- SPDX-License-Identifier: Apache-2.0 -->
# Archived benchmark evidence

These assets cover the historical runs listed below. They do not contain the
[current requalification](README.md#current-requalification), whose raw evidence
has not been uploaded.

Run from the repository root with Python 3.12 or newer:

```sh
python3 tools/verify_benchmark_artifacts.py
```

The command downloads the pinned release asset once, checks its size and SHA-256,
extracts it into a disposable directory under `.local/benchmark-artifacts/`, and
runs the original byte-dispatch, FDW-tuning and Antalya 0.1.1 verifiers. It also
regenerates the ten route-matrix and FDW-control reports. No Rust build, database,
container, Python package installation or private fixture is needed. Original
verifiers check complete cohorts, controls and report reproduction; a checksum
alone is not measurement validation.

The [manifest](artifacts.json) identifies the exact original source commit,
archive URL, compressed and expanded byte counts, member count, and verifier
paths. [SHA256SUMS](SHA256SUMS) pins the downloadable asset. The
[evidence release](https://github.com/f1b0-gh/pg_wildcard/releases/tag/benchmark-evidence-2026-11-23)
contains the complete original 1,200-file results tree, plus LICENSE and NOTICE,
from commit
`0ef4fc1837399278597d381c4741ff4df7edb027`, including the old manifests and harnesses.
Those bytes remain identifiable even if repository history is rewritten.

## Offline use

After the first successful download:

```sh
python3 tools/verify_benchmark_artifacts.py --offline
```

For an air-gapped checkout, copy the published archive onto the machine and run:

```sh
python3 tools/verify_benchmark_artifacts.py --offline --archive /path/to/benchmark-evidence-0ef4fc183739.tar.gz
```

Every use rechecks the cached archive. A missing cache in offline mode fails
with instructions; an interrupted download never installs a partial archive.
A corrupt cache fails closed and can be repaired by importing the correct asset
with `--archive`. Extraction rejects links, unsafe paths, duplicate members and
oversized decompression. The cache is disposable and is excluded from Git.

## Reading the raw evidence

The archive preserves repository-relative paths under
`tests/benchmarks/results/`. Keep a safely extracted copy for inspection with:

```sh
python3 tools/verify_benchmark_artifacts.py --offline --keep
```

The command prints the retained directory only after verification succeeds.
The checked-in result directories retain summaries, settings, harness sources,
provenance and historical hash manifests. Raw samples, generated query sets,
EXPLAIN output, profiler output and logs live in the release archive. Historical
commands shown in result READMEs describe the original measurement procedure;
run the command above to verify their evidence. Directly executing an old
`results/.../verify.py` in the abbreviated checkout cannot find its raw inputs.

## Adding a measurement run

Create a separate deterministic `.tar.gz` asset with regular files beneath
`tests/benchmarks/results/<run>/`, and retain its summary, settings, harness,
manifest and checksums in Git. Use normalized timestamps and ownership, sorted
member names, and gzip with no filename and `mtime=0`. Preserve applicable
LICENSE and NOTICE files when a harness incorporates third-party material.

Copy the schema in [artifacts.json](artifacts.json): `sha256`, `size`, `tar_size`,
`unpacked_size` and `files` describe the archive bytes; `source_commit` records
the measured code; `verifiers` lists archived Python scripts that run from the
extracted repository root. Each verifier must fail on missing or inconsistent
evidence. Set `legacy_reproduction` to false or omit it for new runs. That flag
only invokes the historical November 2026 route/control reproduction recipe.
An empty verifier list checks archive integrity only.

```sh
python3 tools/verify_benchmark_artifacts.py --manifest tests/benchmarks/results/<run>/artifacts.json
```

The manifest and its scripts are reviewed code: only execute trusted manifests.
Publish immutable asset names; an updated measurement gets a new asset and
checksum. Test local imports and report reproduction before publishing, then
test an actual download of the uploaded asset.

## CI and package boundaries

Normal tests and builds do not fetch archived measurements. Automatic CI retains
the generated semantic benchmark smoke tests, and checks the archive tooling
offline. The manually dispatched
[Archived benchmark evidence](../../.github/workflows/benchmark-evidence.yml)
workflow explicitly downloads and reproduces historical evidence.

The automatic size guard reads Git blobs: files under `tests/benchmarks/results/`
must be at most 256 KiB, and a PR's added/modified blobs under that same directory
must total at most 2 MiB. Changes outside benchmark results are not constrained
by this guard. It compares the PR head with its merge base, counts complete
blob sizes rather than line deltas, and never offsets additions with deletions.
Renames count as an addition at the new path. Pure deletions are free. Local check:

```sh
python3 tools/check_artifact_size.py --base BASE_COMMIT --head HEAD_COMMIT
```

The root results directory is outside both published crate roots, `libwildcard/`
and `pg_wildcard/`; this migration reduces checkout size, not their current source
packages. Both manifests exclude package-local `tests/benchmarks/**` as well;
this also removes the library's small Python benchmark harness from its source
package. Keep the library's Puffin regression fixtures and extension fixtures
inside their packages.
