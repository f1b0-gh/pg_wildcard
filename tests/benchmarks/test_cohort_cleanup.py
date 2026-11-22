# SPDX-License-Identifier: Apache-2.0
import threading
import unittest
from unittest.mock import Mock, patch
from compare_routes import cohorts


class CohortCleanup(unittest.TestCase):
    def test_failed_start_barrier_still_joins_observer(self):
        observer = Mock()
        barrier = Mock()
        barrier.wait.side_effect = threading.BrokenBarrierError
        # The cohort fails to start. There must still be a
        # bounded observer shutdown before another measured cohort can run.
        with patch('compare_routes.threading.Thread', return_value=observer), \
             patch('compare_routes.threading.Barrier', return_value=barrier), \
             patch('compare_routes.concurrent.futures.ThreadPoolExecutor'):
            observer.is_alive.return_value = False
            with self.assertRaises(threading.BrokenBarrierError):
                cohorts('native', 1, lambda _: {}, [], None, False, 0,
                        observe=lambda: {})
        observer.join.assert_called_once_with(timeout=3)


if __name__ == '__main__':
    unittest.main()
