<!-- SPDX-License-Identifier: Apache-2.0 -->
# Engine qualification

Complete raw evidence and the original hash manifests are preserved in the
[checksummed release archive](../../../ARTIFACTS.md). From the repository root,
run `python3 tools/verify_benchmark_artifacts.py` to verify this historical run.


The native performance measurements use Antalya 26.8.34.20001. The repository's
pinned native differential fixture uses 26.6.8.20004.altinityantalya; embedded
qualification uses chDB 4.5.1 / ClickHouse 26.11.1.1. These are separate runs.

The minimal [SQL probe](nul-regex-probe.sql), [native response](../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-22/qualification/nul-regex-probe-antalya.json`)
and [embedded response](../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-22/qualification/nul-regex-probe-chdb.json`) expose a pattern portability
limit: a literal NUL regex matches ordinary `a` on the pinned native engine,
but not in the embedded engine. The escaped regex `\x00` matches a NUL-containing
value and rejects `a` on both. Cross-version fixtures use that escaped spelling;
NUL and invalid-UTF-8 **input values** remain exercised. Production code does not
silently normalize patterns. See the [semantic contract](../../../../../docs/semantics.md).

[Native three-seed qualification](../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-22/qualification/native-three-seeds.log`) passed 1,735 / 1,711 /
1,831 predicates for seeds 1 / 42 / 1234. Each seed checked five views over 362
values, ten explicit shared-filter cases, exact sweep/watchlist pairs and stale
regex/view/empty metadata fallbacks. This is semantic qualification, not timing
evidence for the performance tables.

[Embedded three-seed qualification](../../../ARTIFACTS.md#reading-the-raw-evidence) (archive path: `2026-11-22/qualification/chdb-three-seeds.log`) passed the same final
fixtures. The benchmark CI smoke separately passed all sweep and ingest
strategies at 1% scale, with an additional true positive for every indicator in
every sweep strategy. Timing thresholds are intentionally absent.
