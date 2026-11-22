# SPDX-License-Identifier: Apache-2.0
import unittest
from unittest.mock import Mock, patch
from compare_routes import sweeps


class NativeProfile(unittest.TestCase):
    def engine(self):
        engine = sweeps.Engine.__new__(sweeps.Engine)
        engine.state = {'fixture': 'owned'}
        engine.query = Mock()
        return engine

    def test_finish_logged_after_http_completion_is_observed(self):
        engine = self.engine()
        engine.query.side_effect = [[], [], [], [[12, 100, 400, 10, 1000, 4096, {}, {}]]]
        with patch.object(sweeps.time, 'sleep'):
            result = engine.profile('owned_query')
        self.assertEqual(result['read_rows'], 100)
        self.assertEqual(result['result_rows'], 10)

    def test_missing_finish_remains_a_bounded_failure(self):
        engine = self.engine()
        engine.query.return_value = []
        with patch.object(sweeps.time, 'sleep'):
            with self.assertRaisesRegex(AssertionError, 'No QueryFinish'):
                engine.profile('owned_query')
        self.assertLessEqual(engine.query.call_count, 16)


if __name__ == '__main__':
    unittest.main()
