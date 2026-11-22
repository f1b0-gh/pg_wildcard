# FDW terminal policy and batch-budget controls

Baseline release library; persistent connections; one excluded warmup and three measured samples per completed route. Narrow is one million integer rows; wide is 32,768 rows with a 1 KiB payload. Policy tests use 16 MiB; budget tests use in_band. These are small diagnostic controls, not five-pair optimization promotion experiments. Complete Arrow values are checked before timing, full COPY values after timing; backend scans verify row counts.

| Case | Route | Request ms median [range] | Executor ms median [range] | First batch ms median | Terminal ms median | Control requests median | Largest observed individual batch bytes |
|---|---|---:|---:|---:|---:|---:|---:|
| policy-flush_first | direct | 361.396 [267.869, 414.903] | — | 7.478 | 49.711 | 2.000 | 16440 |
| policy-flush_first | pg_backend | 470.979 [389.872, 544.728] | 465.550 [360.724, 523.550] | 8.579 | 43.986 | 2.000 | 16440 |
| policy-flush_first | pg_copy | 643.432 [640.166, 912.383] | — | — | — | — | — |
| policy-observe_first | direct | 585.916 [427.054, 694.392] | — | 20.364 | 138.315 | 3.000 | 128568 |
| policy-observe_first | pg_backend | 1119.730 [775.683, 1120.053] | 1081.289 [758.573, 1106.646] | 18.500 | 178.395 | 3.000 | 80056 |
| policy-observe_first | pg_copy | 1552.615 [1439.256, 1653.121] | — | — | — | — | — |
| policy-observe_only | direct | 1160.420 [1026.819, 1777.692] | — | 16.845 | 829.119 | 9.000 | 131128 |
| policy-observe_only | pg_backend | 1131.537 [878.305, 1431.794] | 1108.383 [868.807, 1415.859] | 5.387 | 571.745 | 7.000 | 115768 |
| policy-observe_only | pg_copy | 1638.249 [1533.065, 1960.606] | — | — | — | — | — |
| policy-in_band | direct | 478.508 [330.445, 549.988] | — | 17.230 | 0.003 | 0.000 | 65992 |
| policy-in_band | pg_backend | 444.313 [366.589, 710.063] | 420.292 [360.452, 698.145] | 5.770 | 0.003 | 0.000 | 65992 |
| policy-in_band | pg_copy | 731.619 [548.309, 734.857] | — | — | — | — | — |

First batch is not network time to first byte. Decode, framing, polling, terminal and backend clocks overlap and are not additive stages. COPY does not expose the backend EXPLAIN diagnostics; its cells remain unavailable. The measured per-batch charge is checked against its configured limit. This does not assert aggregate retained allocations or backend RSS fit within one batch allowance. Sampled backend RSS is retained separately in JSON.

## Explicit resource refusals

- {"batch_bytes": 1048576, "case": "budget-1048576", "classification": "direct value-control data resource refusal before measured samples", "cleanup_confirmed": true, "policy": "in_band", "private_log_sha256": "cdc2bfa8b85d0367c0e96e5cb91740b9a56cf944dc93ced9f3da0e9ca8cfd6c6", "typed_error": "Context { phase: Data, cause: LimitExceeded }"}
- {"batch_bytes": 4194304, "case": "budget-4194304", "classification": "direct value-control data resource refusal before measured samples", "cleanup_confirmed": true, "policy": "in_band", "private_log_sha256": "cdc2bfa8b85d0367c0e96e5cb91740b9a56cf944dc93ced9f3da0e9ca8cfd6c6", "typed_error": "Context { phase: Data, cause: LimitExceeded }"}
- {"batch_bytes": 1048576, "case": "pg-budget-1048576", "classification": "PG full-COPY precontrol data resource refusal before measured samples", "cleanup_confirmed": true, "policy": "in_band", "private_log_sha256": "ef1361eccb4dd9bb65f83abbcc64db6e014cc3c1063b14d7805c7799cc99ade9", "typed_error": "wildcard_fdw: Antalya Data: configured resource limit exceeded"}
- {"batch_bytes": 4194304, "case": "pg-budget-4194304", "classification": "PG full-COPY precontrol data resource refusal before measured samples", "cleanup_confirmed": true, "policy": "in_band", "private_log_sha256": "ef1361eccb4dd9bb65f83abbcc64db6e014cc3c1063b14d7805c7799cc99ade9", "typed_error": "wildcard_fdw: Antalya Data: configured resource limit exceeded"}
