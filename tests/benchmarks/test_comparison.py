# SPDX-License-Identifier: Apache-2.0
import importlib.util
import struct
import unittest
from pathlib import Path

MODULE = Path(__file__).with_name('comparison.py')

class ComparisonContract(unittest.TestCase):
    def setUp(self):
        self.assertTrue(MODULE.exists(), 'shared comparison contract is not implemented')
        spec = importlib.util.spec_from_file_location('comparison', MODULE)
        self.m = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.m)

    def test_multiset_rejects_duplicate_missing_and_later_batch(self):
        expected = [(0, 0, 'raw'), (100, 999, 'lower')]
        self.assertEqual(self.m.verify_pairs(list(reversed(expected)), expected), self.m.pair_digest(expected))
        for broken in [expected[:1], expected + expected[:1], expected[:1] * 2]:
            with self.assertRaises(ValueError): self.m.verify_pairs(broken, expected)

    def test_binary_copy_checks_every_value_and_rejects_truncation(self):
        def row(a, b, view):
            fields = [struct.pack('!q', a), struct.pack('!q', b), view.encode()]
            return struct.pack('!h', 3) + b''.join(struct.pack('!i', len(x)) + x for x in fields)
        raw = b'PGCOPY\n\xff\r\n\0' + struct.pack('!ii', 0, 0) + row(4, 8, 'raw') + struct.pack('!h', -1)
        self.assertEqual(self.m.copy_pairs(raw), [(4, 8, 'raw')])
        for broken in [raw[:-1], raw + b'x', raw[:24], b'x' + raw[1:]]:
            with self.assertRaises(self.m.OracleMismatch): self.m.copy_pairs(broken)

    def test_schedule_has_one_warmup_and_balanced_interleaving(self):
        schedule = self.m.schedule(['native', 'direct', 'pg'], 5)
        self.assertEqual(schedule[:3], [(True, 0, 'native'), (True, 0, 'direct'), (True, 0, 'pg')])
        rounds = [x for x in schedule if not x[0]]
        self.assertEqual(len(rounds), 15)
        self.assertEqual({x[2] for x in rounds[3:6]}, {'native', 'direct', 'pg'})
        self.assertNotEqual([x[2] for x in rounds[:3]], [x[2] for x in rounds[3:6]])

    def test_oracle_preserves_ids_views_and_rare_templates(self):
        spec = {'rows': 11, 'stride': 5, 'templates': ['a', 'b'], 'template_hits': [[[2, 'raw'], [7, 'lower']], [[999, 'raw']]], 'benign_prefix': 'none/'}
        self.assertEqual(self.m.expected_pairs(spec), [(0, 2, 'raw'), (0, 7, 'lower'), (5, 999, 'raw'), (10, 2, 'raw'), (10, 7, 'lower')])
        self.assertEqual(self.m.corpus_value(spec, 5), 'b')
        self.assertEqual(self.m.corpus_value(spec, 6), 'none/6')

    def test_bounds_fail_before_expanding_a_cross_product(self):
        spec = {'rows': 1000000, 'stride': 1, 'templates': ['q'], 'template_hits': [[[i, 'raw'] for i in range(1000)]], 'benign_prefix': 'none/'}
        with self.assertRaises(ValueError): self.m.expected_pairs(spec, max_pairs=100000)
        for n in [0, -1, 21]:
            with self.assertRaises(ValueError): self.m.schedule(['native'], n)

    def test_observable_background_preserves_suffix(self):
        spec = {'rows': 10, 'stride': 5, 'templates': ['hit'],
                'benign_prefix': 'host-', 'benign_suffix': '.example.net'}
        self.assertEqual(self.m.corpus_value(spec, 6), 'host-6.example.net')
        self.assertEqual(self.m.corpus_value(spec, 5), 'hit')

    def test_search_scope_requires_wildcard_field_or_opted_in_observable(self):
        self.assertTrue(hasattr(self.m, 'require_wildcard_target'))
        self.m.require_wildcard_target({'search_target': {'kind': 'field', 'data_type': 'wildcard'}})
        for kind in ['domain', 'url', 'path']:
            self.m.require_wildcard_target({'search_target': {'kind': 'observable',
                'observable_type': kind, 'wildcard_enabled': True}, 'query_contract': 'case_sensitive_wildcard'})
        rejected = [{}, {'kind': 'field'},
                    *({'kind': 'field', 'data_type': t} for t in ['keyword', 'ip', 'long', 'text']),
                    {'kind': 'observable', 'observable_type': 'domain', 'wildcard_enabled': False},
                    {'kind': 'observable', 'observable_type': 'hash', 'wildcard_enabled': True},
                    {'kind': 'observable', 'observable_type': 'sha256', 'wildcard_enabled': True}]
        for target in rejected:
            with self.assertRaises(ValueError):
                self.m.require_wildcard_target({'search_target': target, 'query_contract': 'case_sensitive_wildcard'})
        for missing in [{}, {'search_target': None}, {'search_target': []}]:
            with self.assertRaises(ValueError): self.m.require_wildcard_target(missing)
        with self.assertRaises(ValueError):
            self.m.require_wildcard_target({'search_target': {'kind': 'observable',
                'observable_type': 'domain', 'wildcard_enabled': True}})

    def test_elastic_rejects_partial_success_and_truncated_hits(self):
        good = {'hits': {'total': {'value': 1, 'relation': 'eq'}, 'hits': [{'_source': {'id': 42}}]}, '_shards': {'failed': 0}, 'timed_out': False}
        self.assertTrue(hasattr(self.m, 'elastic_pairs'), 'Elastic completeness oracle missing')
        self.assertEqual(self.m.elastic_pairs([good], [{'id': 9, 'view': 'raw'}]), [(42, 9, 'raw')])
        import copy
        for field in ['timeout', 'shards', 'truncated', 'relation']:
            bad = copy.deepcopy(good)
            if field == 'timeout': bad['timed_out'] = True
            if field == 'shards': bad['_shards']['failed'] = 1
            if field == 'truncated': bad['hits']['hits'] = []
            if field == 'relation': bad['hits']['total']['relation'] = 'gte'
            with self.assertRaises(self.m.OracleMismatch): self.m.elastic_pairs([bad], [{'id': 9, 'view': 'raw'}])

    def test_observed_client_concurrency_uses_intervals_not_requested_workers(self):
        self.assertTrue(hasattr(self.m, 'overlap_peak'), 'observed overlap missing')
        self.assertEqual(self.m.overlap_peak([(0, 1), (1, 2), (2, 3)]), 1)
        self.assertEqual(self.m.overlap_peak([(0, 2), (1, 3), (1.5, 2.5)]), 3)

if __name__ == '__main__': unittest.main()
