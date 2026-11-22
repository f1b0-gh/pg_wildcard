# Index query, write and storage tradeoffs

Completed cases only; the full planned matrix is not complete until every case in matrix-status.json has a terminal outcome. Exact pairs and summary clocks are validated by report-matrix.py before this report is generated. See [the route matrix](matrix-report.md) for direct antalya-rs, PostgreSQL backend and full COPY measurements; they are distinct routes.

## Query observations

Each uncensored route has five measured cohorts after warmup. Request times are client samples; at C4 they are twenty clients nested in five cohorts. Reads, selected marks and query counts are totals per complete cohort divided by its client count, then summarized across cohorts. These are query-log logical reads, not measured disk traffic. Missing counters remain missing. Elastic wildcard and keyword rows have no invented native read counters.

| Case | Route | Request seconds median [range] | Logical rows/client median [range] | Selected marks/client median [range] | Logged queries/client median [range] | Complete cohorts |
|---|---|---:|---:|---:|---:|---:|
| shared-100-c1 | native | 0.603 [0.552, 0.694] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| shared-100-c1 | indexed | 0.593 [0.523, 0.687] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| shared-100-c1 | elastic_wildcard | 0.770 [0.644, 1.045] | — | — | — | 5 |
| shared-100-c1 | elastic_keyword | 3.772 [3.487, 4.074] | — | — | — | 5 |
| unrelated-100-c1 | native | 3.376 [3.141, 3.738] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| unrelated-100-c1 | indexed | 3.524 [2.889, 4.838] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| unrelated-100-c1 | elastic_wildcard | 0.876 [0.725, 2.480] | — | — | — | 5 |
| unrelated-100-c1 | elastic_keyword | 4.536 [4.481, 4.815] | — | — | — | 5 |
| common-100-c1 | native | 13.465 [8.089, 15.776] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| common-100-c1 | indexed | 13.822 [7.816, 15.272] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| common-100-c1 | elastic_wildcard | 42.966 [27.013, 53.253] | — | — | — | 5 |
| common-100-c1 | elastic_keyword | 8.100 [3.694, 14.179] | — | — | — | 5 |
| short-100-c1 | native | 1.793 [1.438, 2.158] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| short-100-c1 | indexed | 1.682 [1.485, 1.976] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| short-100-c1 | elastic_wildcard | 0.825 [0.591, 0.861] | — | — | — | 5 |
| short-100-c1 | elastic_keyword | 0.756 [0.605, 0.787] | — | — | — | 5 |
| mixed-100-c1 | native | 4.075 [3.919, 4.325] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| mixed-100-c1 | indexed | 3.867 [3.621, 4.212] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| shared-1000-c1 | native | 2.631 [2.526, 5.132] | 2,000,000.000 [2,000,000.000, 2,000,000.000] | 246.000 [246.000, 246.000] | 2.000 [2.000, 2.000] | 5 |
| shared-1000-c1 | indexed | 2.828 [2.470, 4.966] | 2,000,000.000 [2,000,000.000, 2,000,000.000] | 246.000 [246.000, 246.000] | 2.000 [2.000, 2.000] | 5 |
| shared-1000-c1 | elastic_wildcard | 2.373 [2.278, 3.327] | — | — | — | 5 |
| shared-1000-c1 | elastic_keyword | 27.654 [27.107, 29.458] | — | — | — | 5 |
| unrelated-1000-c1 | native | 20.313 [18.156, 22.145] | 2,000,000.000 [2,000,000.000, 2,000,000.000] | 246.000 [246.000, 246.000] | 2.000 [2.000, 2.000] | 5 |
| unrelated-1000-c1 | indexed | 21.046 [19.045, 22.527] | 2,000,000.000 [2,000,000.000, 2,000,000.000] | 246.000 [246.000, 246.000] | 2.000 [2.000, 2.000] | 5 |
| unrelated-1000-c1 | elastic_wildcard | 1.880 [1.806, 7.667] | — | — | — | 5 |
| unrelated-1000-c1 | elastic_keyword | 28.631 [27.623, 30.146] | — | — | — | 5 |
| common-1000-c1 | native (censored) | — | — | — | — | 0 |
| common-1000-c1 | indexed (censored) | — | — | — | — | 0 |
| common-1000-c1 | elastic_wildcard (censored) | — | — | — | — | 0 |
| common-1000-c1 | elastic_keyword | 33.312 [32.654, 37.938] | — | — | — | 5 |
| short-1000-c1 | native | 18.893 [17.965, 22.912] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| short-1000-c1 | indexed | 19.467 [18.524, 22.339] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| short-1000-c1 | elastic_wildcard | 4.236 [3.981, 7.764] | — | — | — | 5 |
| short-1000-c1 | elastic_keyword | 4.018 [3.955, 5.720] | — | — | — | 5 |
| mixed-1000-c1 | native | 28.962 [24.976, 38.271] | 7,000,000.000 [7,000,000.000, 7,000,000.000] | 861.000 [861.000, 861.000] | 7.000 [7.000, 7.000] | 5 |
| mixed-1000-c1 | indexed | 26.986 [24.380, 32.246] | 5,000,000.000 [5,000,000.000, 5,000,000.000] | 615.000 [615.000, 615.000] | 5.000 [5.000, 5.000] | 5 |
| shared-1000-c4 | native | 7.619 [6.394, 8.586] | 2,000,000.000 [2,000,000.000, 2,000,000.000] | 246.000 [246.000, 246.000] | 2.000 [2.000, 2.000] | 5 |
| shared-1000-c4 | indexed | 7.324 [6.102, 8.839] | 2,000,000.000 [2,000,000.000, 2,000,000.000] | 246.000 [246.000, 246.000] | 2.000 [2.000, 2.000] | 5 |
| shared-1000-c4 | elastic_wildcard | 1.717 [1.569, 1.983] | — | — | — | 5 |
| shared-1000-c4 | elastic_keyword | 25.957 [24.872, 29.121] | — | — | — | 5 |
| unrelated-1000-c4 | native (censored) | — | — | — | — | 0 |
| unrelated-1000-c4 | indexed (censored) | — | — | — | — | 0 |
| unrelated-1000-c4 | elastic_wildcard | 1.970 [1.721, 3.288] | — | — | — | 5 |
| unrelated-1000-c4 | elastic_keyword | 35.429 [34.189, 41.017] | — | — | — | 5 |
| common-1000-c4 | native (censored) | — | — | — | — | 0 |
| common-1000-c4 | indexed (censored) | — | — | — | — | 0 |
| common-1000-c4 | elastic_wildcard (censored) | — | — | — | — | 0 |
| common-1000-c4 | elastic_keyword | 32.384 [30.532, 48.776] | — | — | — | 5 |
| domains-current-100-c1 | native | 0.584 [0.554, 0.608] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| domains-current-100-c1 | indexed | 0.639 [0.560, 0.940] | 1,000,000.000 [1,000,000.000, 1,000,000.000] | 123.000 [123.000, 123.000] | 1.000 [1.000, 1.000] | 5 |
| domains-current-100-c1 | elastic_wildcard | 0.356 [0.316, 0.426] | — | — | — | 5 |
| domains-current-100-c1 | elastic_keyword | 2.707 [2.498, 2.756] | — | — | — | 5 |
| shared-10000-c1 | native (censored) | 45.774 [36.570, 57.411] | 18,000,000.000 [18,000,000.000, 18,000,000.000] | 2,214.000 [2,214.000, 2,214.000] | 18.000 [18.000, 18.000] | 4 |
| shared-10000-c1 | indexed | 55.506 [34.150, 58.756] | 18,000,000.000 [18,000,000.000, 18,000,000.000] | 2,214.000 [2,214.000, 2,214.000] | 18.000 [18.000, 18.000] | 5 |
| shared-10000-c1 | elastic_wildcard | 16.223 [13.990, 29.019] | — | — | — | 5 |
| shared-10000-c1 | elastic_keyword (censored) | — | — | — | — | 0 |
| unrelated-10000-c1 | native (censored) | — | — | — | — | 0 |
| unrelated-10000-c1 | indexed (censored) | — | — | — | — | 0 |
| unrelated-10000-c1 | elastic_wildcard | 13.466 [12.976, 13.905] | — | — | — | 5 |
| unrelated-10000-c1 | elastic_keyword (censored) | — | — | — | — | 0 |

An index appearing in EXPLAIN does not prove useful pruning. Compare actual rows and marks; a query can read the same marks and still differ in CPU/time. The indexed layout also uses materialized view columns, so generated SQL and batch counts can differ. Fewer total reads caused by fewer source queries must not be attributed to index pruning. For the completed mixed-1000 C1 case, native uses seven queries and indexed native five; all 35 native and 25 indexed measured query profiles read one million rows and 123 marks each. These are complete layout comparisons, not an isolated index-on/off control. Censored routes do not establish a complete five-cohort performance comparison. Fixtures are compared within each case only. In particular, short/no-gram fixtures repeat one literal under multiple IDs, and mixed views have no Elastic equivalence claim. Keyword is a historical storage control, not an admitted application keyword field.

## Setup and insertion observations

These are one setup observation per layout and case, not five repeated write trials. Native initial load creates 90% of rows; indexed setup copies those rows (copy time was not recorded), then adds/materializes its index. Backfill includes materialized view columns where specified. Incremental insertion adds the remaining 10%, followed by OPTIMIZE FINAL outside the insert clock. Elastic bulk clocks include client generation/serialization and HTTP writes; refresh and force-merge occur afterwards. They are not exclusive engine write times and are not directly comparable to native INSERT SELECT CPU.

| Case | Layout | Initial load s | Index backfill s | Incremental insert s | Total bytes | Data compressed bytes | Index compressed bytes |
|---|---|---:|---:|---:|---:|---:|---:|
| shared-100-c1 | bench | 0.472 | — | 0.075 | 9,236,873 | 9,228,714 | 0 |
| shared-100-c1 | bench_indexed | — | 2.035 | 0.270 | 14,374,888 | 9,228,714 | 5,137,865 |
| shared-100-c1 | elastic_wildcard | 74.753 | — | 7.557 | 82,258,140 | — | — |
| shared-100-c1 | elastic_keyword | 64.874 | — | 3.915 | 34,057,985 | — | — |
| unrelated-100-c1 | bench | 0.410 | — | 0.087 | 9,383,916 | 9,375,755 | 0 |
| unrelated-100-c1 | bench_indexed | — | 1.207 | 0.264 | 15,273,987 | 9,375,755 | 5,889,921 |
| unrelated-100-c1 | elastic_wildcard | 74.711 | — | 8.633 | 67,878,459 | — | — |
| unrelated-100-c1 | elastic_keyword | 62.783 | — | 8.317 | 33,354,663 | — | — |
| common-100-c1 | bench | 0.349 | — | 0.054 | 9,258,635 | 9,250,489 | 0 |
| common-100-c1 | bench_indexed | — | 1.342 | 0.211 | 14,432,841 | 9,250,489 | 5,174,056 |
| common-100-c1 | elastic_wildcard | 72.798 | — | 5.637 | 94,844,791 | — | — |
| common-100-c1 | elastic_keyword | 103.662 | — | 7.166 | 25,612,876 | — | — |
| short-100-c1 | bench | 0.831 | — | 0.141 | 9,238,337 | 9,230,178 | 0 |
| short-100-c1 | bench_indexed | — | 1.402 | 0.442 | 13,722,393 | 9,230,178 | 4,483,906 |
| short-100-c1 | elastic_wildcard | 122.844 | — | 14.387 | 65,291,090 | — | — |
| short-100-c1 | elastic_keyword | 39.312 | — | 4.233 | 32,983,932 | — | — |
| mixed-100-c1 | bench | 0.554 | — | 0.086 | 9,376,436 | 9,368,275 | 0 |
| mixed-100-c1 | bench_indexed | — | 8.184 | 0.924 | 49,935,230 | 25,451,939 | 24,462,202 |
| shared-1000-c1 | bench | 0.252 | — | 0.072 | 9,237,106 | 9,228,947 | 0 |
| shared-1000-c1 | bench_indexed | — | 1.416 | 0.171 | 14,347,930 | 9,228,947 | 5,110,674 |
| shared-1000-c1 | elastic_wildcard | 38.088 | — | 4.499 | 135,130,728 | — | — |
| shared-1000-c1 | elastic_keyword | 33.930 | — | 3.770 | 33,980,685 | — | — |
| unrelated-1000-c1 | bench | 0.384 | — | 0.051 | 9,383,542 | 9,375,381 | 0 |
| unrelated-1000-c1 | bench_indexed | — | 0.909 | 0.296 | 15,554,355 | 9,375,381 | 6,170,663 |
| unrelated-1000-c1 | elastic_wildcard | 50.425 | — | 4.117 | 67,918,390 | — | — |
| unrelated-1000-c1 | elastic_keyword | 51.901 | — | 7.155 | 33,287,093 | — | — |
| common-1000-c1 | bench | 0.341 | — | 0.050 | 9,259,519 | 9,251,369 | 0 |
| common-1000-c1 | bench_indexed | — | 1.330 | 0.202 | 14,439,212 | 9,251,369 | 5,179,543 |
| common-1000-c1 | elastic_wildcard | 39.584 | — | 3.672 | 70,341,768 | — | — |
| common-1000-c1 | elastic_keyword | 60.975 | — | 6.555 | 54,074,090 | — | — |
| short-1000-c1 | bench | 0.406 | — | 0.069 | 9,238,337 | 9,230,178 | 0 |
| short-1000-c1 | bench_indexed | — | 0.986 | 0.282 | 13,722,393 | 9,230,178 | 4,483,906 |
| short-1000-c1 | elastic_wildcard | 43.075 | — | 4.497 | 65,258,185 | — | — |
| short-1000-c1 | elastic_keyword | 34.275 | — | 3.393 | 24,363,707 | — | — |
| mixed-1000-c1 | bench | 0.335 | — | 0.055 | 9,376,680 | 9,368,519 | 0 |
| mixed-1000-c1 | bench_indexed | — | 4.377 | 0.673 | 51,082,473 | 25,455,240 | 25,606,143 |
| shared-1000-c4 | bench | 0.511 | — | 0.087 | 9,237,106 | 9,228,947 | 0 |
| shared-1000-c4 | bench_indexed | — | 0.989 | 0.301 | 14,347,930 | 9,228,947 | 5,110,674 |
| shared-1000-c4 | elastic_wildcard | 50.551 | — | 4.561 | 67,594,567 | — | — |
| shared-1000-c4 | elastic_keyword | 34.936 | — | 3.500 | 65,682,764 | — | — |
| unrelated-1000-c4 | bench | 0.256 | — | 0.061 | 9,383,542 | 9,375,381 | 0 |
| unrelated-1000-c4 | bench_indexed | — | 1.314 | 0.179 | 15,554,355 | 9,375,381 | 6,170,663 |
| unrelated-1000-c4 | elastic_wildcard | 33.708 | — | 3.622 | 57,020,498 | — | — |
| unrelated-1000-c4 | elastic_keyword | 34.836 | — | 3.687 | 33,287,661 | — | — |
| common-1000-c4 | bench | 0.357 | — | 0.046 | 9,259,519 | 9,251,369 | 0 |
| common-1000-c4 | bench_indexed | — | 0.685 | 0.185 | 14,439,212 | 9,251,369 | 5,179,543 |
| common-1000-c4 | elastic_wildcard | 40.147 | — | 4.153 | 60,029,150 | — | — |
| common-1000-c4 | elastic_keyword | 33.054 | — | 3.505 | 34,169,337 | — | — |
| domains-current-100-c1 | bench | 0.258 | — | 0.043 | 9,287,410 | 9,279,282 | 0 |
| domains-current-100-c1 | bench_indexed | — | 0.578 | 0.178 | 17,762,225 | 9,279,282 | 8,474,665 |
| domains-current-100-c1 | elastic_wildcard | 52.829 | — | 9.666 | 60,554,486 | — | — |
| domains-current-100-c1 | elastic_keyword | 39.772 | — | 3.780 | 37,520,658 | — | — |
| shared-10000-c1 | bench | 0.336 | — | 0.063 | 9,237,221 | 9,229,061 | 0 |
| shared-10000-c1 | bench_indexed | — | 1.255 | 0.167 | 14,289,185 | 9,229,061 | 5,051,814 |
| shared-10000-c1 | elastic_wildcard | 40.340 | — | 4.377 | 57,152,241 | — | — |
| shared-10000-c1 | elastic_keyword | 33.936 | — | 3.535 | 34,037,793 | — | — |
| unrelated-10000-c1 | bench | 0.562 | — | 0.064 | 9,383,731 | 9,375,564 | 0 |
| unrelated-10000-c1 | bench_indexed | — | 4.000 | 0.245 | 15,589,712 | 9,375,564 | 6,205,831 |
| unrelated-10000-c1 | elastic_wildcard | 46.318 | — | 4.871 | 67,914,230 | — | — |
| unrelated-10000-c1 | elastic_keyword | 33.789 | — | 3.786 | 24,385,048 | — | — |

Total storage describes whole layouts, not comparable isolated index overhead. Native mixed-view data includes additional materialized columns; the difference in total bytes cannot all be attributed to the index. Exact DDL and raw engine statistics remain in each case directory and matrix-storage.json.

The separately reported bounded raw-byte index is a different private layout. Its five paired write trials must not be substituted for these production text-index observations. The current evidence does not justify automatically migrating every wildcard field to that prototype.

Shared-host load and filesystem caches were uncontrolled; retain the reported ranges, session boundaries and settings rather than treating small differences as a general engine ranking.
