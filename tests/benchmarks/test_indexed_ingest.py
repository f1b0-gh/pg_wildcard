# SPDX-License-Identifier: Apache-2.0
import copy
import json
import unittest

from pathlib import Path
import importlib


class IndexedIngest(unittest.TestCase):
    def setUp(self):
        self.assertTrue(Path(__file__).with_name("indexed_ingest.py").is_file(), "public indexed wire-ingest support is missing")
        global indexed_ingest
        indexed_ingest = importlib.import_module("indexed_ingest")

    def spec(self):
        return {'rows': 11, 'stride': 5, 'templates': ['a"\\\nb', 'xy'],
                'template_hits': [[[2, 'raw']], [[7, 'raw']]], 'benign_prefix': 'none/',
                'queries': ['old query'], 'patterns': [{'id': 2}, {'id': 7}],
                'diagnostics': {'source_queries': 2},
                'byte_dispatch': {'strategy': 'byte_dispatch', 'fallback': None,
                                  'queries': ['new query'], 'estimated_ast': [42]}}

    def test_layout_preserves_the_oracle_and_does_not_mutate_the_input(self):
        original = self.spec(); saved = copy.deepcopy(original)
        selected = indexed_ingest.select_spec(original)
        self.assertEqual(selected['queries'], ['new query'])
        for key in ('patterns', 'templates', 'template_hits', 'rows'):
            self.assertEqual(selected[key], original[key])
        self.assertEqual(selected['diagnostics']['source_queries'], 1)
        self.assertEqual(original, saved)
        for fallback in ('Coverage', 'QueryBudget'):
            original['byte_dispatch']['fallback'] = fallback
            with self.assertRaises(ValueError): indexed_ingest.select_spec(original)

    def test_http_encodings_deliver_identical_documents_in_both_phases(self):
        decoded = []
        for elastic in (False, True):
            phases = list(indexed_ingest.phases(self.spec(), elastic))
            self.assertEqual([(name, stop) for name, stop, _, _ in phases], [('initial', 9), ('incremental', 11)])
            docs = []
            for _, _, chunks, _ in phases:
                for body, count in chunks:
                    lines = body.splitlines()
                    if elastic:
                        actions = [json.loads(v) for v in lines[::2]]
                        batch = [json.loads(v) for v in lines[1::2]]
                        self.assertEqual([a['index']['_id'] for a in actions], [str(v['id']) for v in batch])
                    else:
                        self.assertEqual(lines[0], 'INSERT INTO bench FORMAT JSONEachRow')
                        batch = [json.loads(v) for v in lines[1:]]
                    self.assertEqual(count, len(batch)); docs.extend(batch)
            self.assertEqual([v['id'] for v in docs], list(range(11)))
            self.assertEqual(docs[0]['v'], 'a"\\\nb')
            self.assertEqual(docs[5]['v'], 'xy')
            decoded.append(docs)
        self.assertEqual(*decoded)
        for rows in (0, -1, 1_000_001):
            with self.assertRaises(ValueError): list(indexed_ingest.phases(dict(self.spec(), rows=rows), False))


if __name__ == '__main__': unittest.main()
