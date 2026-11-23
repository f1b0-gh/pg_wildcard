# FDW overhead evidence

Five alternating baseline/optimized pairs, one measured observation per route and workload in each cohort. Warmups are excluded. Values below are median [minimum, maximum] across five observations.

| Workload | Route / clock | Baseline ms | Optimized ms | Paired reduction median [range] |
|---|---|---:|---:|---:|
| narrow | direct complete Arrow | 258.203 [247.944, 418.198] | 241.609 [232.703, 382.664] | 8.497 [-16.018, 10.290]% |
| narrow | PG request round trip | 311.784 [256.427, 585.053] | 287.124 [242.477, 590.580] | 7.648 [-0.945, 10.068]% |
| narrow | PG backend executor | 304.116 [250.752, 577.172] | 281.055 [237.541, 582.992] | 7.583 [-1.008, 9.928]% |
| narrow | COPY complete delivery | 373.833 [352.910, 601.465] | 360.983 [322.301, 786.327] | 3.437 [-30.735, 33.236]% |
| smalltext | direct complete Arrow | 870.644 [786.000, 1108.092] | 902.921 [801.219, 950.184] | -6.191 [-14.085, 18.516]% |
| smalltext | PG request round trip | 843.030 [801.317, 926.784] | 874.992 [806.877, 932.262] | -8.051 [-11.785, 10.059]% |
| smalltext | PG backend executor | 817.683 [779.244, 906.811] | 857.648 [788.413, 910.257] | -8.572 [-12.186, 9.615]% |
| smalltext | COPY complete delivery | 972.849 [950.922, 1433.713] | 884.172 [843.112, 953.001] | 9.601 [1.349, 38.330]% |
| wide1k | direct complete Arrow | 493.365 [463.963, 832.739] | 503.772 [433.216, 629.309] | -1.067 [-35.638, 39.504]% |
| wide1k | PG request round trip | 543.965 [471.266, 743.345] | 521.467 [508.917, 561.614] | 6.443 [-10.652, 24.448]% |
| wide1k | PG backend executor | 531.226 [460.637, 727.658] | 507.643 [495.576, 547.385] | 6.711 [-10.205, 24.774]% |
| wide1k | COPY complete delivery | 583.152 [552.465, 909.877] | 526.777 [508.023, 693.190] | 11.953 [-21.559, 44.166]% |
| broad_recheck | direct complete Arrow | 839.705 [762.417, 953.035] | 849.169 [745.406, 1023.102] | 3.473 [-34.192, 11.230]% |
| broad_recheck | PG request round trip | 754.456 [723.776, 999.403] | 688.291 [642.682, 775.755] | 14.815 [-7.182, 31.130]% |
| broad_recheck | PG backend executor | 730.697 [706.559, 976.219] | 665.390 [620.424, 755.522] | 15.091 [-6.930, 31.840]% |
| broad_recheck | COPY complete delivery | 1223.668 [1140.565, 1437.329] | 988.994 [859.052, 1226.339] | 24.682 [-1.273, 31.192]% |

The direct route is a distinct transport control; subtracting its wall clock from PG time does not isolate CPU overhead.

| Workload | Binary | Samples | Any unresolved frame | User CPU µs/row | System CPU µs/row | Non-matcher sampled share | Non-matcher user CPU proxy µs/row |
|---|---|---:|---:|---:|---:|---:|---:|
| narrow | baseline | 1611 | 813 (50.5%) | 0.2985 | 0.0955 | 100.0% | 0.2985 |
| smalltext | baseline | 3885 | 1231 (31.7%) | 0.8000 | 0.1895 | 100.0% | 0.8000 |
| wide1k | baseline | 663 | 163 (24.6%) | 3.4637 | 0.9155 | 100.0% | 3.4637 |
| broad_recheck | baseline | 3893 | 1912 (49.1%) | 0.9165 | 0.0490 | 93.0% | 0.8522 |
| narrow | optimized | 1382 | 684 (49.5%) | 0.2675 | 0.0980 | 100.0% | 0.2675 |
| smalltext | optimized | 3072 | 1071 (34.9%) | 0.6045 | 0.1590 | 100.0% | 0.6045 |
| wide1k | optimized | 649 | 206 (31.7%) | 3.3112 | 0.8545 | 100.0% | 3.3112 |
| broad_recheck | optimized | 2910 | 1469 (50.5%) | 0.6635 | 0.0480 | 90.7% | 0.6015 |

Non-matcher sampled share = weight of stacks with neither a visible Matcher::is_match frame nor a regex, regex_automata, or aho_corasick frame / total folded stack weight. The intended boundary excludes the whole Matcher::is_match call, including its view/scratch work; engine frames also identify matcher samples when that caller is inlined or missing. The JSON reports complements of visible Matcher::is_match alone and engine frames alone separately, plus their union used for the proxy. This complement includes PostgreSQL, FDW, transport, conversion, wrapper, view/cache work, unresolved frames, and matcher work hidden by inlining or incomplete unwinding. It is not an exclusive stage measurement. Both binaries use perf --no-inline and exact saved ELF symbols: recorded DWARF-unwound frames have no synthetic inline-frame expansion. Therefore even fully resolved ELF symbols do not reveal all inlined Matcher::is_match work; the complements cannot exactly isolate everything outside that call. Estimated non-matcher user CPU/row = process user CPU/row times that share. cycles:u sampling and process CPU ticks measure different quantities; this multiplication is an attribution proxy, not exact CPU accounting. System CPU is reported separately and is not apportioned by user-only samples.

| Workload | Non-matcher proxy reduction | At least 50% in point estimate? |
|---|---:|---|
| narrow | 10.4% | no |
| smalltext | 24.4% | no |
| wide1k | 4.4% | no |
| broad_recheck | 29.4% | no |

**The half-overhead target is not established by these single sampled profiles.** The table reports whether the defined proxy reached 50% in this observation; it does not establish halving of true non-matcher CPU.

Verification checked complete cohort ordering, row counts and persisted exact-value controls, matching runner/settings/binary identities, source summaries against raw measurements, twenty plans per profile, and identical folded stacks reconstructed from demangled samples.

Binary SHA256:

- baseline: `a625b3f57403153e67f19dfe30e5373b942f05285e326518501bdb9af92fb844`
- optimized: `6cdc02fff366ac2c55ebc8d6082f523e01de569112f6aff06761596a8237d763`

Recorded host load (median [range]):

| Binary | 1 minute | 5 minutes | 15 minutes |
|---|---:|---:|---:|
| baseline | 8.081 [5.896, 14.597] | 10.360 [8.295, 11.503] | 12.195 [10.736, 12.845] |
| optimized | 7.166 [6.316, 12.351] | 9.616 [7.930, 10.569] | 11.811 [10.438, 12.502] |

Host 1/5/15-minute load averages recorded after each measured route sample; warmups excluded; observations are correlated and are not CPU utilization.

Limits:

- Five alternating pairs; warmups excluded. Paired ratios use cohort index, not pooled medians. No significance claim.
- Direct control consumes Arrow batches; PG executor is EXPLAIN ANALYZE TIMING OFF; COPY includes full binary delivery. These are different scopes.
- Profile CPU includes profiling overhead and twenty scans. Do not compare it to unprofiled wall time as if clocks were interchangeable.
- Sample categories are inclusive and overlap. Inlining, unresolved frames, and truncated stacks can hide matcher work.
- Any-unresolved counts include system-library and PostgreSQL frames, not just extension frames. A stack counts once even with several unresolved frames; unresolved work cannot be assigned confidently.
- Both profiles use perf --no-inline with exact saved ELF symbols and DWARF unwinding; no synthetic inline expansion. Resolved symbols alone cannot recover inlined Matcher::is_match boundaries.
- Single CPU profile per workload/binary cannot establish a robust halving of true non-matcher overhead.
- Shared-host background activity is not controlled by this report; host-load ranges describe recorded conditions, not causal attribution.
- Exact-value controls are persisted runner attestations; raw COPY/Arrow payloads are not retained for independent value replay.
- SHA consistency does not itself prove build reproducibility; source commits and flags are recorded in build-provenance.json.
