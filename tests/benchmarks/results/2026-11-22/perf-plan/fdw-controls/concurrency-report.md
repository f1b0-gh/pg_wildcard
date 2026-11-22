# FDW concurrency observations

C1 contains five separate baseline rounds. C8 and C32 each contain one measured barrier-started cohort per workload/route after a warmup; clients within a cohort are not independent repetitions. This is an observed scaling/resource control, not a five-repetition concurrency promotion gate. Rows are fully consumed. Wide means 32,768 rows of 1 KiB payload per client; narrow means one million integer rows per client.

| Requested clients | Workload | Route | Measured cohorts | Client samples | Client request ms median [range] | Cohort rows/s median [range] | Observed clients / database-referencing queries | Largest backend RSS MiB | Largest sampled sum of backend RSS MiB | Largest sampled sum of backend threads |
|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | narrow | direct | 5 | 5 | 322.2 [308.1, 512.1] | 3092476.9 [1944843.0, 3230505.3] | 1 / 1 | — | — | — |
| 1 | narrow | pg_backend | 5 | 5 | 345.5 [297.9, 928.3] | 2891119.4 [1076820.8, 3352923.7] | 1 / 1 | 28.6 | 28.6 | 2.0 |
| 1 | narrow | pg_copy | 5 | 5 | 512.9 [404.9, 768.8] | 1949196.5 [1300481.3, 2468880.2] | 1 / 1 | 29.0 | 29.0 | 2.0 |
| 1 | wide1k | direct | 5 | 5 | 634.6 [573.4, 1137.2] | 51606.4 [28485.3, 57119.9] | 1 / 1 | — | — | — |
| 1 | wide1k | pg_backend | 5 | 5 | 665.5 [607.1, 838.5] | 49212.1 [39065.7, 53952.4] | 1 / 1 | 58.6 | 58.6 | 2.0 |
| 1 | wide1k | pg_copy | 5 | 5 | 705.1 [615.7, 968.0] | 46469.1 [33848.0, 53212.7] | 1 / 1 | 62.9 | 62.9 | 2.0 |
| 8 | narrow | direct | 1 | 8 | 932.3 [841.9, 992.4] | 7934932.7 [7934932.7, 7934932.7] | 8 / 8 | — | — | — |
| 8 | narrow | pg_backend | 1 | 8 | 1085.9 [804.3, 1234.1] | 6480071.8 [6480071.8, 6480071.8] | 8 / 8 | 24.7 | 195.9 | 16.0 |
| 8 | narrow | pg_copy | 1 | 8 | 1922.0 [1641.9, 2008.3] | 3982371.4 [3982371.4, 3982371.4] | 8 / 8 | 24.8 | 196.8 | 16.0 |
| 8 | wide1k | direct | 1 | 8 | 1991.5 [1794.2, 2091.4] | 124848.9 [124848.9, 124848.9] | 8 / 8 | — | — | — |
| 8 | wide1k | pg_backend | 1 | 8 | 2225.0 [1476.0, 2511.3] | 104348.1 [104348.1, 104348.1] | 8 / 8 | 60.5 | 451.5 | 16.0 |
| 8 | wide1k | pg_copy | 1 | 8 | 2835.6 [2033.6, 3041.3] | 86168.0 [86168.0, 86168.0] | 8 / 8 | 65.0 | 456.2 | 16.0 |
| 32 | narrow | direct | 1 | 32 | 5766.0 [2964.0, 6629.7] | 4796328.2 [4796328.2, 4796328.2] | 32 / 32 | — | — | — |
| 32 | narrow | pg_backend | 1 | 32 | 8401.7 [6960.1, 8913.8] | 3587826.4 [3587826.4, 3587826.4] | 32 / 32 | 28.7 | 903.9 | 64.0 |
| 32 | narrow | pg_copy | 1 | 32 | 15291.4 [11139.1, 16807.8] | 1902965.6 [1902965.6, 1902965.6] | 32 / 32 | 28.7 | 904.8 | 64.0 |
| 32 | wide1k | direct | 1 | 32 | 11907.8 [7027.2, 13117.2] | 79914.1 [79914.1, 79914.1] | 32 / 32 | — | — | — |
| 32 | wide1k | pg_backend | 1 | 32 | 13241.1 [8475.8, 14093.5] | 74203.3 [74203.3, 74203.3] | 32 / 33 | 71.5 | 1714.9 | 64.0 |
| 32 | wide1k | pg_copy | 1 | 32 | 12948.5 [7636.7, 14984.3] | 69930.3 [69930.3, 69930.3] | 32 / 32 | 71.9 | 1837.9 | 64.0 |

Throughput divides complete cohort rows by earliest client start to last client completion. Request clocks retain each route's existing boundary. The server sampler counts query text referencing this invocation's database and excludes itself. It can include diagnostic queries, so it is not an exact source-scan concurrency measure: the C32 wide backend control observes 33 database-referencing queries while only 32 client requests are outstanding. The raw count is retained rather than capped or relabeled as 33 clients. Resource peaks are sampled observations, not proven absolute maxima. Backend RSS sums use processes from the same sample; shared pages can be counted multiple times, so they are not physical memory totals. Direct-client process RSS/threads were not instrumented; idle PostgreSQL processes are deliberately excluded from that route's resource cells. Neither RSS measure is bounded by the individual batch allowance. Raw settings, active-query samples, per-backend samples and query profiles remain in each run directory. No clocks are subtracted into exclusive stages.

The original C32 attempt was interrupted by a host reboot and is preserved separately with its damaged tail and incomplete metadata. Only the fresh, fully validated C32 run appears in this table. C1/C8 were measured before that reboot and C32 afterwards, on a shared host. These are separate scaling observations, not a controlled single-session scaling curve or proof that a throughput difference is caused only by concurrency. No cold-cache claim is made.
