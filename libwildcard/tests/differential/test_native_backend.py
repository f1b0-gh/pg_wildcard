# SPDX-License-Identifier: Apache-2.0
import os
import unittest
from unittest.mock import patch

from native_backend import NativeSession


class OwnershipTests(unittest.TestCase):
    def test_benchmark_fixture_requires_explicit_selection(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(ValueError):
                NativeSession('wildcard-benchmark-antalya-0123456789abcdef')

    def test_selected_benchmark_fixture_requires_matching_owner_label(self):
        name = 'wildcard-benchmark-antalya-0123456789abcdef'
        for labels in ('{}', '{"wildcard.benchmark.owner":"fedcba9876543210"}'):
            with self.subTest(labels=labels), patch.dict(os.environ, {'ANTALYA_TEST_CONTAINER': name}), \
                    patch('native_backend.subprocess.check_output', return_value=labels):
                with self.assertRaisesRegex(ValueError, 'ownership'):
                    NativeSession(name)


if __name__ == '__main__':
    unittest.main()
