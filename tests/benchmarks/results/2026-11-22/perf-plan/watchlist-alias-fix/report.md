# Watchlist helper-projection transfer check

Five interleaved pairs after warmup, one million source rows and 1,000 rules per fixture. Both variants use the same previous production rule join; only the source helper projection changes. The complete native INSERT SELECT persists all hit pairs through the actual MV. Exact validation is outside the timer. This is not a new regex or Elastic comparison.

| Fixture | Before fix ms median [range] | Isolated projection ms median [range] | Median ratio change | Median paired change |
|---|---:|---:|---:|---:|
| shared | 4992.559 [4598.944, 6092.553] | 4837.078 [4641.552, 5183.696] | -3.11% | -3.11% |
| unrelated | 4743.550 [4439.621, 5257.049] | 4754.207 [4620.622, 4939.349] | +0.22% | +0.22% |

The source/rules are identical to the earlier production-transfer fixtures. Each measured request returns the exact 10,000-pair multiset and has an original native profile. Host load and filesystem caches remain uncontrolled; this small transfer check does not establish a general speedup from the correctness fix. A median regression above 10% requires investigation. Historical timings and failed controls remain unchanged.
