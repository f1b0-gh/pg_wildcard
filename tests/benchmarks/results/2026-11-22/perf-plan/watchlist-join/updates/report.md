# Watchlist update and subsequent ingestion costs

Each trial starts from the same watchlist and rotates existing rule definitions among stable IDs. The global pattern set is unchanged. The update clock covers native HTTP upserts in batches of 128, excluding SQL generation. A separate clock measures the first resident-source insertion after visibility validation, including source and hit persistence. Neither clock is a PostgreSQL or direct antalya-rs measurement. No Elastic percolator comparison is made.

| Family | Rules | Requested block rows | Strategy | Update ms median [range] | Rules/s | Subsequent 10k-row insert ms median [range] | Rows/s | Update CPU ms median | Insert CPU ms median | Largest update / insert query MiB |
|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|
| domains | 100 | 1024 | staged_needles | 69.19 [68.86, 70.22] | 1445 | 124.56 [108.51, 153.33] | 80283 | 1.21 | 122.05 | 1.09 / 53.87 |
| domains | 100 | 1024 | production_join | 68.59 [58.85, 70.48] | 1458 | 73.84 [57.67, 76.19] | 135431 | 0.99 | 68.27 | 1.09 / 14.95 |
| domains | 100 | 8192 | staged_needles | 68.90 [59.15, 69.54] | 1451 | 135.53 [122.65, 161.51] | 73784 | 1.15 | 133.12 | 1.09 / 53.87 |
| domains | 100 | 8192 | production_join | 69.07 [58.83, 70.25] | 1448 | 63.56 [53.25, 68.43] | 157324 | 1.19 | 60.40 | 1.09 / 14.95 |
| shared | 1000 | 1024 | staged_needles | 500.15 [489.05, 560.18] | 1999 | 987.19 [934.44, 1021.79] | 10130 | 8.46 | 983.84 | 1.09 / 684.06 |
| shared | 1000 | 1024 | production_join | 520.27 [518.39, 543.27] | 1922 | 78.26 [76.30, 82.23] | 127776 | 10.52 | 74.28 | 1.09 / 21.78 |
| shared | 1000 | 8192 | staged_needles | 494.69 [489.70, 519.24] | 2021 | 1020.47 [968.03, 1166.17] | 9799 | 9.64 | 1013.72 | 1.09 / 684.06 |
| shared | 1000 | 8192 | production_join | 512.81 [483.45, 541.53] | 1950 | 82.61 [67.75, 92.72] | 121057 | 9.62 | 79.78 | 1.09 / 21.78 |

Five interleaved measured pairs per case; warmups excluded. Complete row/indicator/view multisets agree after every update and subsequent insertion. Requested block settings are recorded, not a measured distribution of internal block sizes. Throughput divides the fixed operation count by its median latency. CPU sums each update request within one trial; memory is the largest individual query peak, not total process RSS. Both strategies write the same watchlist schema; any update-latency difference is a control, not evidence of a cheaper update algorithm. New-pattern compilation and concurrent update visibility require different experiments. Raw query profiles, SQL, settings and snapshots remain beside each case.
