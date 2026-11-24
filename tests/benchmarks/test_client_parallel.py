# SPDX-License-Identifier: Apache-2.0
import tempfile
from pathlib import Path
import unittest
from unittest.mock import Mock, patch
from client_parallel import assert_clean, cleanup, profiles, server_time

class CleanupTests(unittest.TestCase):
    def test_close_failure_does_not_skip_other_owners_or_fixture_teardown(self):
        a=Mock();a.close.side_effect=RuntimeError('close failure');b=Mock();engine=Mock()
        with tempfile.TemporaryDirectory() as directory, patch('client_parallel.cancel_native_queries') as cancel, patch('client_parallel.fixtures.stop') as stop:
            out=Path(directory)
            errors=cleanup([a,b],engine,out/'state',out,False)
            self.assertEqual(len(errors),1);b.close.assert_called_once();cancel.assert_called_once_with(engine)
            a.process.wait.assert_called_once_with(timeout=5)
            engine.close.assert_called_once();stop.assert_called_once_with(out/'state')

    def test_unknown_execution_forces_teardown_even_when_sql_cleanup_succeeds(self):
        engine=Mock()
        with tempfile.TemporaryDirectory() as directory, patch('client_parallel.cancel_native_queries'), patch('client_parallel.fixtures.stop') as stop:
            out=Path(directory)
            self.assertEqual(cleanup([],engine,out/'state',out,True),[])
            engine.close.assert_called_once();stop.assert_called_once()

class ObserverTests(unittest.TestCase):
    def engine(self):
        return Mock(database='wc_benchmark_'+'a'*32,prefix='wc_benchmark_'+'a'*32+'_')

    def test_observers_cannot_be_mistaken_for_sweep_work(self):
        engine=self.engine()
        engine.query.side_effect=[[[123]],[],[],[],[[0]]]
        self.assertEqual(server_time(engine),123)
        self.assertEqual(profiles(engine,100,200),[])
        assert_clean(engine)
        ids=[call.kwargs['query_id'] for call in engine.query.call_args_list]
        self.assertEqual(len(ids),len(set(ids)))
        for call in engine.query.call_args_list:
            self.assertFalse(call.kwargs['query_id'].startswith(engine.prefix))
            self.assertFalse(call.kwargs['query_id'].startswith('comparison-direct-'))
            self.assertFalse(call.kwargs['logging'])

    def test_real_active_work_still_fails_with_identity(self):
        engine=self.engine()
        engine.query.side_effect=[[['comparison-direct-active',1.0,'SELECT sleep(2)']],[[0]]]
        with self.assertRaisesRegex(AssertionError,'comparison-direct-active'):
            assert_clean(engine)

if __name__=='__main__':unittest.main()
