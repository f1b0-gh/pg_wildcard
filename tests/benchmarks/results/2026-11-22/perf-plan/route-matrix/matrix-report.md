# Comparable route matrix — completed cases

Each row is a separately measured route on the same admitted wildcard fixture. Request medians/ranges use complete consumption, with correctness checks outside that clock. PostgreSQL backend request includes EXPLAIN transport; its TIMING OFF executor clock is reported separately. Native HTTP, direct published antalya-rs, PostgreSQL backend, full binary COPY and Elastic are not interchangeable timers. Keyword is a historical Elastic storage-layout control, not application field admission.

Five barrier-started measured cohorts follow one warmup for uncensored routes. At concurrency four, twenty clients are nested in five cohorts; they are not twenty independent repetitions. Filesystem caches are uncontrolled. Engine resources, host load, limits and precise SQL/DSL remain in each case directory. Cases spanning the host reboot are separate sessions, not paired code comparisons.

| Case | Route | Complete cohorts / client samples | Request ms median [range] | Executor ms median [range] | Observed clients / engine searches | Status |
|---|---|---:|---:|---:|---:|---|
| shared-100-c1 | native | 5 / 5 | 602.67 [551.72, 693.86] | — | 1 / 1 | complete |
| shared-100-c1 | indexed | 5 / 5 | 593.08 [523.29, 687.29] | — | 1 / 1 | complete |
| shared-100-c1 | direct | 5 / 5 | 620.09 [572.88, 715.02] | — | 1 / 1 | complete |
| shared-100-c1 | pg_backend | 5 / 5 | 740.48 [573.46, 1712.52] | 732.48 [570.42, 1710.17] | 1 / 1 | complete |
| shared-100-c1 | pg_copy | 5 / 5 | 1499.14 [562.42, 1630.26] | — | 1 / 1 | complete |
| shared-100-c1 | elastic_wildcard | 5 / 5 | 770.05 [643.72, 1044.65] | — | 1 / 1 | complete |
| shared-100-c1 | elastic_keyword | 5 / 5 | 3772.46 [3486.59, 4074.01] | — | 1 / 1 | complete |
| unrelated-100-c1 | native | 5 / 5 | 3375.89 [3140.87, 3738.20] | — | 1 / 1 | complete |
| unrelated-100-c1 | indexed | 5 / 5 | 3524.16 [2889.47, 4838.47] | — | 1 / 1 | complete |
| unrelated-100-c1 | direct | 5 / 5 | 3348.69 [2945.63, 3896.73] | — | 1 / 1 | complete |
| unrelated-100-c1 | pg_backend | 5 / 5 | 3803.36 [3736.91, 4425.29] | 3798.48 [3731.70, 4421.85] | 1 / 1 | complete |
| unrelated-100-c1 | pg_copy | 5 / 5 | 3522.99 [3385.61, 4767.42] | — | 1 / 1 | complete |
| unrelated-100-c1 | elastic_wildcard | 5 / 5 | 876.23 [724.73, 2480.26] | — | 1 / 1 | complete |
| unrelated-100-c1 | elastic_keyword | 5 / 5 | 4535.68 [4481.38, 4814.75] | — | 1 / 1 | complete |
| common-100-c1 | native | 5 / 5 | 13464.90 [8088.84, 15775.84] | — | 1 / 1 | complete |
| common-100-c1 | indexed | 5 / 5 | 13821.60 [7816.06, 15272.28] | — | 1 / 1 | complete |
| common-100-c1 | direct | 5 / 5 | 14728.94 [9957.62, 15398.20] | — | 1 / 1 | complete |
| common-100-c1 | pg_backend | 5 / 5 | 12561.26 [11718.97, 14494.39] | 12553.51 [11714.00, 14479.39] | 1 / 1 | complete |
| common-100-c1 | pg_copy | 5 / 5 | 13579.03 [11445.90, 15594.78] | — | 1 / 1 | complete |
| common-100-c1 | elastic_wildcard | 5 / 5 | 42966.48 [27013.14, 53253.01] | — | 1 / 1 | complete |
| common-100-c1 | elastic_keyword | 5 / 5 | 8100.49 [3693.97, 14179.10] | — | 1 / 1 | complete |
| short-100-c1 | native | 5 / 5 | 1793.18 [1438.42, 2158.34] | — | 1 / 1 | complete |
| short-100-c1 | indexed | 5 / 5 | 1682.43 [1484.51, 1976.37] | — | 1 / 1 | complete |
| short-100-c1 | direct | 5 / 5 | 1852.69 [1784.43, 2294.01] | — | 1 / 1 | complete |
| short-100-c1 | pg_backend | 5 / 5 | 1847.67 [1461.05, 2277.96] | 1838.69 [1459.17, 2273.40] | 1 / 1 | complete |
| short-100-c1 | pg_copy | 5 / 5 | 1659.76 [1461.69, 2116.28] | — | 1 / 1 | complete |
| short-100-c1 | elastic_wildcard | 5 / 5 | 825.49 [591.11, 860.99] | — | 1 / 1 | complete |
| short-100-c1 | elastic_keyword | 5 / 5 | 755.83 [604.65, 786.91] | — | 1 / 1 | complete |
| mixed-100-c1 | native | 5 / 5 | 4075.26 [3919.45, 4325.47] | — | 1 / 1 | complete |
| mixed-100-c1 | indexed | 5 / 5 | 3867.09 [3620.74, 4211.78] | — | 1 / 1 | complete |
| mixed-100-c1 | direct | 5 / 5 | 4831.20 [3912.80, 4993.67] | — | 1 / 1 | complete |
| mixed-100-c1 | pg_backend | 5 / 5 | 4874.94 [4846.09, 5688.11] | 4871.68 [4843.04, 5682.12] | 1 / 1 | complete |
| mixed-100-c1 | pg_copy | 5 / 5 | 4567.31 [3700.25, 4740.63] | — | 1 / 1 | complete |
| shared-1000-c1 | native | 5 / 5 | 2630.58 [2525.64, 5131.75] | — | 1 / 1 | complete |
| shared-1000-c1 | indexed | 5 / 5 | 2827.90 [2469.81, 4966.25] | — | 1 / 1 | complete |
| shared-1000-c1 | direct | 5 / 5 | 4783.08 [4491.68, 5740.76] | — | 1 / 1 | complete |
| shared-1000-c1 | pg_backend | 5 / 5 | 4664.28 [4512.91, 6366.11] | 4657.54 [4506.97, 6356.94] | 1 / 1 | complete |
| shared-1000-c1 | pg_copy | 5 / 5 | 4580.53 [4524.43, 7014.10] | — | 1 / 1 | complete |
| shared-1000-c1 | elastic_wildcard | 5 / 5 | 2373.04 [2278.04, 3326.57] | — | 1 / 1 | complete |
| shared-1000-c1 | elastic_keyword | 5 / 5 | 27653.60 [27107.46, 29457.66] | — | 1 / 1 | complete |
| unrelated-1000-c1 | native | 5 / 5 | 20312.51 [18155.65, 22144.59] | — | 1 / 1 | complete |
| unrelated-1000-c1 | indexed | 5 / 5 | 21046.18 [19045.24, 22527.38] | — | 1 / 1 | complete |
| unrelated-1000-c1 | direct | 5 / 5 | 20772.45 [19535.33, 26449.93] | — | 1 / 1 | complete |
| unrelated-1000-c1 | pg_backend | 5 / 5 | 20756.80 [19619.50, 22612.29] | 20749.57 [19612.50, 22604.20] | 1 / 1 | complete |
| unrelated-1000-c1 | pg_copy | 5 / 5 | 20502.45 [19442.58, 25491.25] | — | 1 / 1 | complete |
| unrelated-1000-c1 | elastic_wildcard | 5 / 5 | 1879.58 [1806.08, 7666.73] | — | 1 / 1 | complete |
| unrelated-1000-c1 | elastic_keyword | 5 / 5 | 28631.00 [27622.73, 30146.28] | — | 1 / 1 | complete |
| common-1000-c1 | native | 0 / 0 | — | — | 1 / 1 | censored; partial observations only |
| common-1000-c1 | indexed | 0 / 0 | — | — | 1 / 1 | censored; partial observations only |
| common-1000-c1 | direct | 0 / 0 | — | — | 1 / 1 | censored; partial observations only |
| common-1000-c1 | pg_backend | 0 / 0 | — | — | None / None | censored; partial observations only |
| common-1000-c1 | pg_copy | 0 / 0 | — | — | None / None | censored; partial observations only |
| common-1000-c1 | elastic_wildcard | 0 / 0 | — | — | 1 / 1 | censored; partial observations only |
| common-1000-c1 | elastic_keyword | 5 / 5 | 33311.86 [32654.12, 37938.47] | — | 1 / 1 | complete |
| short-1000-c1 | native | 5 / 5 | 18893.08 [17965.42, 22911.82] | — | 1 / 1 | complete |
| short-1000-c1 | indexed | 5 / 5 | 19467.27 [18523.55, 22339.46] | — | 1 / 1 | complete |
| short-1000-c1 | direct | 5 / 5 | 19690.37 [18882.01, 22513.13] | — | 1 / 1 | complete |
| short-1000-c1 | pg_backend | 5 / 5 | 19943.17 [19078.18, 22670.31] | 19940.35 [19075.16, 22643.41] | 1 / 1 | complete |
| short-1000-c1 | pg_copy | 5 / 5 | 19568.50 [18565.36, 24215.71] | — | 1 / 1 | complete |
| short-1000-c1 | elastic_wildcard | 5 / 5 | 4236.37 [3981.23, 7763.97] | — | 1 / 1 | complete |
| short-1000-c1 | elastic_keyword | 5 / 5 | 4018.05 [3955.20, 5719.99] | — | 1 / 1 | complete |
| mixed-1000-c1 | native | 5 / 5 | 28961.98 [24976.33, 38271.48] | — | 1 / 1 | complete |
| mixed-1000-c1 | indexed | 5 / 5 | 26986.25 [24379.96, 32245.82] | — | 1 / 1 | complete |
| mixed-1000-c1 | direct | 5 / 5 | 32930.61 [31169.24, 41108.79] | — | 1 / 1 | complete |
| mixed-1000-c1 | pg_backend | 5 / 5 | 40435.12 [31920.79, 41008.84] | 40414.46 [31900.71, 40984.69] | 1 / 1 | complete |
| mixed-1000-c1 | pg_copy | 5 / 5 | 36731.72 [33089.70, 41970.65] | — | 1 / 1 | complete |
| shared-1000-c4 | native | 5 / 20 | 7618.88 [6393.69, 8585.80] | — | 4 / 4 | complete |
| shared-1000-c4 | indexed | 5 / 20 | 7324.47 [6101.90, 8839.41] | — | 4 / 4 | complete |
| shared-1000-c4 | direct | 5 / 20 | 7938.41 [6867.50, 9887.99] | — | 4 / 4 | complete |
| shared-1000-c4 | pg_backend | 5 / 20 | 8186.21 [6880.21, 10224.79] | 8180.09 [6874.36, 10219.33] | 4 / 4 | complete |
| shared-1000-c4 | pg_copy | 5 / 20 | 8021.02 [6585.26, 9817.80] | — | 4 / 4 | complete |
| shared-1000-c4 | elastic_wildcard | 5 / 20 | 1716.63 [1568.99, 1983.19] | — | 4 / 4 | complete |
| shared-1000-c4 | elastic_keyword | 5 / 20 | 25956.75 [24871.84, 29120.90] | — | 4 / 4 | complete |
| unrelated-1000-c4 | native | 0 / 0 | — | — | 4 / 4 | censored; partial observations only |
| unrelated-1000-c4 | indexed | 0 / 0 | — | — | 4 / 4 | censored; partial observations only |
| unrelated-1000-c4 | direct | 0 / 0 | — | — | 4 / 4 | censored; partial observations only |
| unrelated-1000-c4 | pg_backend | 0 / 0 | — | — | 4 / 4 | censored; partial observations only |
| unrelated-1000-c4 | pg_copy | 0 / 0 | — | — | None / None | censored; partial observations only |
| unrelated-1000-c4 | elastic_wildcard | 5 / 20 | 1969.83 [1721.18, 3287.86] | — | 4 / 4 | complete |
| unrelated-1000-c4 | elastic_keyword | 5 / 20 | 35428.99 [34188.87, 41016.55] | — | 4 / 4 | complete |
| common-1000-c4 | native | 0 / 0 | — | — | 4 / 4 | censored; partial observations only |
| common-1000-c4 | indexed | 0 / 0 | — | — | 4 / 4 | censored; partial observations only |
| common-1000-c4 | direct | 0 / 0 | — | — | 4 / 4 | censored; partial observations only |
| common-1000-c4 | pg_backend | 0 / 0 | — | — | None / None | censored; partial observations only |
| common-1000-c4 | pg_copy | 0 / 0 | — | — | None / None | censored; partial observations only |
| common-1000-c4 | elastic_wildcard | 0 / 0 | — | — | 4 / 4 | censored; partial observations only |
| common-1000-c4 | elastic_keyword | 5 / 20 | 32383.70 [30532.45, 48775.57] | — | 4 / 4 | complete |
| domains-current-100-c1 | native | 5 / 5 | 584.21 [554.33, 608.18] | — | 1 / 1 | complete |
| domains-current-100-c1 | indexed | 5 / 5 | 639.06 [559.86, 939.53] | — | 1 / 1 | complete |
| domains-current-100-c1 | direct | 5 / 5 | 644.49 [590.73, 1661.50] | — | 1 / 1 | complete |
| domains-current-100-c1 | pg_backend | 5 / 5 | 1706.95 [617.42, 1727.53] | 1704.27 [614.63, 1724.44] | 1 / 1 | complete |
| domains-current-100-c1 | pg_copy | 5 / 5 | 1503.47 [577.95, 1603.27] | — | 1 / 1 | complete |
| domains-current-100-c1 | elastic_wildcard | 5 / 5 | 355.93 [315.72, 426.06] | — | 1 / 1 | complete |
| domains-current-100-c1 | elastic_keyword | 5 / 5 | 2707.37 [2497.72, 2756.21] | — | 1 / 1 | complete |
| shared-10000-c1 | native | 4 / 4 | 45773.80 [36569.62, 57411.02] | — | 1 / 1 | censored; partial observations only |
| shared-10000-c1 | indexed | 5 / 5 | 55505.85 [34150.01, 58756.31] | — | 1 / 1 | complete |
| shared-10000-c1 | direct | 3 / 3 | 47786.10 [45694.51, 48442.08] | — | 1 / 1 | censored; partial observations only |
| shared-10000-c1 | pg_backend | 1 / 1 | 51908.96 [51908.96, 51908.96] | 51856.25 [51856.25, 51856.25] | 1 / 1 | censored; partial observations only |
| shared-10000-c1 | pg_copy | 0 / 0 | — | — | 1 / 1 | censored; partial observations only |
| shared-10000-c1 | elastic_wildcard | 5 / 5 | 16222.64 [13990.06, 29018.82] | — | 1 / 1 | complete |
| shared-10000-c1 | elastic_keyword | 0 / 0 | — | — | 1 / 1 | censored; partial observations only |
| unrelated-10000-c1 | native | 0 / 0 | — | — | 1 / 1 | censored; partial observations only |
| unrelated-10000-c1 | indexed | 0 / 0 | — | — | 1 / 1 | censored; partial observations only |
| unrelated-10000-c1 | direct | 0 / 0 | — | — | 1 / 1 | censored; partial observations only |
| unrelated-10000-c1 | pg_backend | 0 / 0 | — | — | None / None | censored; partial observations only |
| unrelated-10000-c1 | pg_copy | 0 / 0 | — | — | None / None | censored; partial observations only |
| unrelated-10000-c1 | elastic_wildcard | 5 / 5 | 13465.83 [12976.32, 13904.53] | — | 1 / 1 | complete |
| unrelated-10000-c1 | elastic_keyword | 0 / 0 | — | — | 1 / 1 | censored; partial observations only |

Censored rows preserve any completed observations but do not establish a five-repetition speed ratio. Unsupported semantic subsets and bounded stress refusals are listed below. A backend-only sample checks executor rows against SQL whose full COPY pairs were validated before measurement; it does not deliver those pairs to the client during its clock.

The JSON retains query-log reads/CPU for all clients in each cohort, not an assumed scan count. Query count is separate from rows/marks read. Engine peak is a sampled active-search count, not verified disk parallelism. Elastic uses one inner search at a time per msearch; requested client concurrency is a separate setting. Client-sweep throughput in JSON uses the external cohort makespan, including protocol handling and any worker-side decoding outside the inner request timer. It must not be described as isolated engine throughput.

## Fixture diversity and output fanout

Indicator count is not necessarily distinct pattern count. The short/no-gram fixture deliberately repeats one exact single-character pattern, `q`, under different IDs. It tests empty-gram fallback and preservation of every matching ID; it does not represent a diverse short-wildcard watchlist. Do not extrapolate its route ordering to general short patterns or regex engines. Exact output pairs, source selectivity and pattern diversity are separate dimensions.

| Case | Rows | Indicator entries | Unique regex/view pairs | Template values | Expected output pairs | Elastic shared subset |
|---|---:|---:|---:|---:|---:|---|
| shared-100-c1 | 1000000 | 100 | 100 | 100 | 10000 | True |
| unrelated-100-c1 | 1000000 | 100 | 100 | 100 | 10000 | True |
| common-100-c1 | 1000000 | 100 | 100 | 100 | 10000 | True |
| short-100-c1 | 1000000 | 100 | 1 | 1 | 10000 | True |
| mixed-100-c1 | 1000000 | 100 | 100 | 100 | 10000 | False |
| shared-1000-c1 | 1000000 | 1000 | 1000 | 1000 | 10000 | True |
| unrelated-1000-c1 | 1000000 | 1000 | 1000 | 1000 | 10000 | True |
| common-1000-c1 | 1000000 | 1000 | 1000 | 1000 | 10000 | True |
| short-1000-c1 | 1000000 | 1000 | 1 | 1 | 100000 | True |
| mixed-1000-c1 | 1000000 | 1000 | 1000 | 1000 | 10000 | False |
| shared-1000-c4 | 1000000 | 1000 | 1000 | 1000 | 10000 | True |
| unrelated-1000-c4 | 1000000 | 1000 | 1000 | 1000 | 10000 | True |
| common-1000-c4 | 1000000 | 1000 | 1000 | 1000 | 10000 | True |
| domains-current-100-c1 | 1000000 | 100 | 100 | 200 | 5000 | True |
| shared-10000-c1 | 1000000 | 10000 | 10000 | 10000 | 10000 | True |
| unrelated-10000-c1 | 1000000 | 10000 | 10000 | 10000 | 10000 | True |

## Explicit omissions and bounded outcomes

- {"case": "mixed-100-c1", "kind": "excluded", "reason": "mixed views and RE2 alternation are native/Rust qualification only", "route": "elastic"}
- {"attempt_directory": ".local/perf/matrix-setup-failures/common-1000-c1-pg-timeout", "case": "common-1000-c1", "classification": "PostgreSQL full COPY precontrol statement timeout; original collector aborted all routes before measured cohorts", "cleanup_confirmed": true, "exit_status": 1, "seconds": 203.63599973799955, "status": "failed"}
- {"case": "common-1000-c1", "cleanup_confirmed": true, "error": "PostgreSQL statement timeout", "kind": "precontrol_failure", "routes": ["pg_backend", "pg_copy"], "stage": "full_copy", "timed": false, "worker": 0}
- {"case": "mixed-1000-c1", "kind": "excluded", "reason": "mixed views and RE2 alternation are native/Rust qualification only", "route": "elastic"}
- {"case": "common-1000-c4", "cleanup_confirmed": true, "error": "PostgreSQL statement timeout", "kind": "precontrol_failure", "routes": ["pg_backend", "pg_copy"], "stage": "full_copy", "timed": false, "worker": 0}
- {"case": "unrelated-10000-c1", "cleanup_confirmed": true, "error": "PostgreSQL statement timeout", "kind": "precontrol_failure", "routes": ["pg_backend", "pg_copy"], "stage": "full_copy", "timed": false, "worker": 0}
- {"case": "common-10000-c1", "generator_timeout_seconds": 60, "reason": "bounded stress fixture generation did not finish; no query timing or correctness claim", "status": "censored_generation_budget"}
- {"case": "short-10000-c1", "error": "oracle exceeds pair budget", "spec_sha256": "60fd3fed2758162701cb3d7a5b481444f51cd740455f7355e358309f992a21e3", "status": "censored_pair_budget"}
- {"case": "mixed-10000-c1", "generator_timeout_seconds": 60, "reason": "bounded stress fixture generation did not finish; no query timing or correctness claim", "status": "censored_generation_budget"}

This report covers only cases marked complete in matrix-status.json. It does not infer completion of the full matrix. Build/insert/storage observations are retained separately in matrix-storage.json, with per-case layout labels.
