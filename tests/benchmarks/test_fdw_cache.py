# SPDX-License-Identifier: Apache-2.0
import unittest
from fdw_cache import capacity_summary

class CacheEvidenceTests(unittest.TestCase):
    def test_capacity_summary_excludes_cold_and_warmup_work(self):
        records=[dict(phase='capacity_hot',cache_hits=int(i<32),probes=0 if i<32 else 2,request_ms=5) for i in range(40)]
        records += [dict(phase='capacity_cold',cache_hits=0,probes=2,request_ms=100)]*40
        records += [dict(phase='capacity_warmup',cache_hits=0,probes=2,request_ms=100)]*8
        summary=capacity_summary(records,20)
        self.assertEqual(summary['hot_hit_rate'],.8)
        self.assertEqual(summary['hot_probes'],16)
        self.assertEqual(summary['hot_median_ms'],5)

    def test_expiry_or_incomplete_round_cannot_support_eviction_claim(self):
        records=[dict(phase='capacity_hot',cache_hits=1,probes=0,request_ms=5)]*40
        self.assertRaises(ValueError,capacity_summary,records,60)
        self.assertRaises(ValueError,capacity_summary,records[:-1],20)
        self.assertRaises(ValueError,capacity_summary,records,-1)

if __name__=='__main__':unittest.main()
