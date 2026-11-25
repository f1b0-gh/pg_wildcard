# SPDX-License-Identifier: Apache-2.0
import unittest
from compare_routes import direct_timing

class DirectTimingTests(unittest.TestCase):
    def test_delivery_clock_includes_ipc_and_retains_client_clock(self):
        result=direct_timing({'request_ms':10,'worker_roundtrip_ms':17,'pairs':[[1,2,'raw']]},'delivery')
        self.assertEqual(result['request_ms'],17)
        self.assertEqual(result['client_request_ms'],10)
        self.assertEqual(result['pairs'],[[1,2,'raw']])
        self.assertEqual(direct_timing({'request_ms':10,'worker_roundtrip_ms':17},'internal')['request_ms'],10)

    def test_failed_or_unconfirmed_execution_is_never_a_sample(self):
        for reply in ({'error':'private failure','cleanup_confirmed':False},
                      {'request_ms':10,'worker_roundtrip_ms':17,'cleanup_confirmed':False}):
            with self.assertRaisesRegex(RuntimeError,'direct execution failed'):
                direct_timing(reply,'delivery')

    def test_delivery_limit_includes_last_reply_and_serialization(self):
        with self.assertRaises(TimeoutError):
            direct_timing({'request_ms':59900,'worker_roundtrip_ms':60001},'delivery')


class ReapingTests(unittest.TestCase):
    def test_close_failure_still_kills_and_reaps_worker(self):
        from unittest.mock import Mock
        from compare_routes import close_worker
        worker=Mock();worker.close.side_effect=RuntimeError('close failed');worker.process.poll.return_value=None
        with self.assertRaisesRegex(RuntimeError,'close failed'):close_worker(worker)
        worker.process.kill.assert_called_once();worker.process.wait.assert_called_once_with(timeout=5)

if __name__=='__main__':unittest.main()
