# FDW CPU samples

Twenty backend scans per workload, cycles:u at 199 Hz with DWARF unwinding. A sample counts once in a category when any frame matches its rule. Categories overlap: tuple building is inside the FDW callback, and Arrow/Base64 can be inside Antalya. These percentages cannot be added or treated as wall-time stages. A falling percentage is not by itself proof of an end-to-end speedup.

| Binary | Workload | Samples | FDW scan callback | Tuple-building context | Explicit value/Datum conversion frames | Local wc_* wrapper | antalya-rs | Arrow IPC | Base64 |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| baseline | narrow | 1428 | 39.3% | 14.6% | 7.7% | 0.0% | 23.5% | 2.9% | 9.5% |
| baseline | smalltext | 3636 | 53.7% | 40.8% | 21.6% | 0.0% | 30.1% | 3.9% | 12.3% |
| baseline | wide1k | 672 | 24.0% | 13.8% | 10.7% | 0.0% | 64.3% | 0.1% | 32.9% |
| baseline | broad_recheck | 3783 | 39.3% | 32.2% | 18.6% | 24.3% | 20.3% | 1.3% | 9.5% |
| prepared | narrow | 1356 | 35.4% | 8.1% | 0.1% | 0.0% | 25.1% | 4.5% | 8.7% |
| prepared | smalltext | 3251 | 47.4% | 33.0% | 0.2% | 0.0% | 34.3% | 5.4% | 13.5% |
| prepared | wide1k | 646 | 21.8% | 11.5% | 0.0% | 0.0% | 66.7% | 0.3% | 33.9% |
| prepared | broad_recheck | 3584 | 32.3% | 21.3% | 0.1% | 29.4% | 20.7% | 1.5% | 9.8% |

The scan callback and tuple-building context are covered by [conversion follow-up #17](https://github.com/f1b0-gh/pg_wildcard/issues/17); [streaming #18](https://github.com/f1b0-gh/pg_wildcard/issues/18) covers the transport path. Local wc_* wrappers include view handling and matcher/cache lookup, not just regex execution. Regex-engine changes remain a paper discussion.

The explicit conversion-frame category can undercount inlined conversion work. Use the actual folded stacks and flamegraphs when assessing its boundaries. The prepared conversion prototype is rejected because the five paired latency trials did not establish acceptable backend and complete-delivery gains across the workloads, even where sampled tuple-building cost fell.

Original symbolization through /proc/1/root left extension frames unresolved. The same recordings were reprocessed with stable symbol directories and exact saved binaries. All eight sample totals are unchanged; five extension frames remain unresolved across the two broad-recheck profiles, and zero in the other six. See symbolization.json. No query was replayed to repair the symbols.
