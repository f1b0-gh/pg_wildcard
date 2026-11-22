# Production transfer: balanced initial order

Native MV insertion of 1M rows with 1,000 unrelated rules. Original and reverse-start cohorts are both retained. Warmups are excluded; all ten measured pairs contribute. This compares prototype and production SQL, not regex engines or staged-baseline speedup.

| Cohort | Pairs | Prototype median [range] s | Production median [range] s | Ratio of medians | Paired change median [range] |
|---|---:|---:|---:|---:|---:|
| original | 5 | 5.964 [5.415, 7.170] | 6.670 [5.575, 7.354] | +11.85% | 2.953 [-1.268, 11.846]% |
| reverse | 5 | 5.011 [4.616, 5.582] | 4.885 [4.610, 5.253] | -2.51% | -2.514 [-7.248, 2.176]% |
| all_samples | 10 | 5.499 [4.616, 7.170] | 5.414 [4.610, 7.354] | -1.54% | 1.744 [-7.248, 11.846]% |

The original +11.85% ratio remains part of this evidence. Balancing the initial order reduces that specific ordering imbalance; it does not prove that all differences came from noise or control unrelated host activity.

Investigation still required: False. This diagnostic flag is not a production-promotion decision; correctness, updates, fallbacks, memory, other distributions and whole-change review remain separate gates.
