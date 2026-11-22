# SPDX-License-Identifier: Apache-2.0
"""Small deterministic checks for supplement boundaries, without services."""
import json
import sys
import unittest

import fdw_adaptive
import fdw_wire


class SupplementTests(unittest.TestCase):
    def test_copy_oracle_counts_fields_and_variable_length_urls(self):
        self.assertEqual(fdw_wire.expected_bytes('narrow'), 14000021)
        self.assertEqual(fdw_wire.expected_bytes('broad_recheck'), 56000221)
        with self.assertRaises(ValueError):
            fdw_wire.copy_query('narrow', 'events; DROP SERVER x')
        with self.assertRaises(ValueError):
            fdw_wire.expected_bytes('unknown')

    def test_copy_rejects_complete_header_with_truncated_rows(self):
        producer = [sys.executable, '-c',
                    'import sys; sys.stdin.buffer.read(); '
                    'sys.stdout.buffer.write(bytes.fromhex("' +
                    (fdw_wire.HEADER + fdw_wire.TRAILER).hex() + '"))']
        result = fdw_wire.consume_copy(producer, 'SELECT 1', 'narrow', 1)
        self.assertTrue(result['header_valid'])
        self.assertTrue(result['trailer_valid'])
        self.assertEqual(result['copy_bytes'], 21)
        self.assertIn('oracle mismatch', result['error'])

    def test_adaptive_decodes_three_analyzed_plans_from_one_session(self):
        plans = [{'Plan': {'Node Type': 'Foreign Scan', 'sample': i}}
                 for i in range(3)]
        output = '1234\n' + '\n'.join(json.dumps([p], indent=2) for p in plans)
        pid, decoded = fdw_adaptive.decode_plans(output)
        self.assertEqual(pid, 1234)
        self.assertEqual(decoded, plans)
        with self.assertRaises(ValueError):
            fdw_adaptive.decode_plans('1234\n' + json.dumps([plans[0]]))


if __name__ == '__main__':
    unittest.main()
