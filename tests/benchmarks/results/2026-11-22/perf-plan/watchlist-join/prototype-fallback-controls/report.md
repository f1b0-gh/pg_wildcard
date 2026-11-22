# Unfavorable native watchlist controls

10,000 source rows × 1,000 rules; native INSERT SELECT including source and hit persistence. Each completed insertion is checked against the full exact native match multiset. Warmups are excluded. Query limit: 120 seconds. This is not a million-row, direct-client, PostgreSQL or Elastic comparison.

| Case | Completed samples staged / join | Paired rounds | Paired staged seconds [range] | Paired join seconds [range] | Paired change median [range] | Largest observed completed-query MiB staged / join |
|---|---:|---:|---:|---:|---:|---:|
| no_grams | 3 / 5 | 3 | 102.769 [90.974–104.741] | 91.285 [86.407–100.451] | -5.020 [-11.175–-4.096]% | 302.00 / 7.99 |
| common_gram | 5 / 5 | 5 | 4.036 [3.864–6.624] | 3.661 [3.355–3.737] | -10.280 [-43.594–-7.327]% | 366.05 / 7.99 |
| stale_bindings | 5 / 5 | 5 | 5.458 [5.177–6.095] | 4.755 [4.723–4.835] | -11.555 [-21.983–-8.546]% | 302.00 / 7.98 |
| long_values | 0 / 5 | 0 | — | — | — | — / 16.43 |

Only matching successful repetition numbers enter the paired timing cells. A case with fewer than five pairs is incomplete/censored; its partial paired summary is descriptive, not the full promotion gate. Memory cells show the largest observed completed measured query, with unequal sample counts retained. Failures are not successful latency observations, and failed output is not used as an oracle. The runner confirms timed-out queries absent before truncating source/hit tables for the next strategy.

## Successful series without a paired baseline

- long_values, rule_join: 5 completed samples; 5.503 [5.414–5.821] seconds. No baseline speedup ratio is available.

## Preserved failures

- no_grams, staged_needles, warmup=False, repeat=3: Code: 159. DB::Exception: Timeout exceeded: elapsed 121930.556 ms, maximum: 120000.000 ms. (TIMEOUT_EXCEEDED) (version 26.8.34.20001.altinityantalya) Server absent after cancellation: True.
- long_values, staged_needles, warmup=True, repeat=0: Code: 241. DB::Exception: Query memory limit exceeded: would use 4.06 GiB (attempt to allocate chunk of 3.91 GiB), maximum: 3.73 GiB: while executing 'FUNCTION position(__table2.v :: 0, arrayElement(_CAST(7883091326372909755_6275670435763007268_Array(String), 'Array(String)'_String), i... :: 4) -> position(__table2.v, arrayElement(_CAST(7883091326372909755_6275670435763007268_Array(String), 'Arra... UInt64 : 2': while executing 'FUNCTION arrayFilter(i UInt32 -> greater(position(__table2.v, arrayElement(_CAST(7883091326372909755_6275670435763007268_... :: 4, _CAST(7389132973273086103_10351578196622134393_Array(UInt32), 'Array(UInt32)'_String) :: 7) -> arrayFilter(i UInt32 -> greater(position(__table2.v, arrayElement(_CAST(7883091326372909755_62756704... Array(UInt32) : 5': while pushing to view wc_benchmark_523279db50fe4a7fa31b65a3db333569.ingest_mv. (MEMORY_LIMIT_EXCEEDED) (version 26.8.34.20001.altinityantalya) Server absent after cancellation: True.
