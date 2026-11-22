# SPDX-License-Identifier: Apache-2.0
import unittest
from unittest.mock import Mock
import compare_routes
from comparison import OracleMismatch


class PgPrecontrol(unittest.TestCase):
    def test_timeout_cleans_every_session_and_cancels_before_censoring(self):
        first, second, cancel = Mock(), Mock(), Mock()
        first.request.side_effect=TimeoutError('PostgreSQL statement timeout')
        events=[]
        first.close.side_effect=lambda:events.append('close-first')
        second.close.side_effect=lambda:events.append('close-second')
        cancel.side_effect=lambda:events.append('cancel')
        result=compare_routes.pg_precontrol([first,second],'full',[],'canary',[],cancel)
        self.assertEqual(result['stage'],'full_copy')
        self.assertEqual(result['worker'],0)
        self.assertEqual(result['error'],'PostgreSQL statement timeout')
        self.assertTrue(result['cleanup_confirmed'])
        self.assertEqual(events,['close-first','close-second','cancel'])
        second.request.assert_not_called()

    def test_exact_pair_failure_is_not_a_censored_timeout(self):
        worker,cancel=Mock(),Mock()
        worker.request.return_value={'pairs':[]}
        with self.assertRaises(OracleMismatch):
            compare_routes.pg_precontrol([worker],'full',[(1,2,'raw')],'canary',[],cancel)
        cancel.assert_not_called()

    def test_unexpected_worker_failure_remains_fatal(self):
        worker,cancel=Mock(),Mock();worker.request.side_effect=RuntimeError('unexpected')
        with self.assertRaisesRegex(RuntimeError,'unexpected'):
            compare_routes.pg_precontrol([worker],'full',[],'canary',[],cancel)

    def test_cleanup_failure_cannot_be_recorded_as_confirmed(self):
        worker,cancel=Mock(),Mock();worker.request.side_effect=TimeoutError('deadline')
        cancel.side_effect=RuntimeError('cancellation failed')
        with self.assertRaisesRegex(RuntimeError,'cancellation failed'):
            compare_routes.pg_precontrol([worker],'full',[],'canary',[],cancel)

    def test_success_checks_full_results_and_canaries_on_every_worker(self):
        workers=[Mock(),Mock()];cancel=Mock()
        for worker in workers:worker.request.side_effect=[{'pairs':[(1,2,'raw')]},{'pairs':[(3,2,'raw')]}]
        self.assertIsNone(compare_routes.pg_precontrol(workers,'full',[(1,2,'raw')],'canary',[(3,2,'raw')],cancel))
        for worker in workers:self.assertEqual(worker.request.call_count,2);worker.close.assert_not_called()
        cancel.assert_not_called()


if __name__=='__main__':unittest.main()
