# SPDX-License-Identifier: Apache-2.0
"""Opt-in native regression: WILDCARD_BENCHMARK_STATE must name an owned fixture."""
import concurrent.futures
import json
import os
from pathlib import Path
import subprocess
import time
import unittest

from compare_routes import cancel_native_queries, sweeps


@unittest.skipUnless(os.environ.get('WILDCARD_BENCHMARK_STATE'), 'owned native fixture required')
class ProcessIsolationTests(unittest.TestCase):
    def test_observation_and_cancellation_use_the_query_database(self):
        state_path=os.environ['WILDCARD_BENCHMARK_STATE']
        state=json.loads(Path(state_path).read_text())
        self.assertRegex(state['name'],r'^wildcard-benchmark-antalya-[a-f0-9]{16}$')
        info=json.loads(subprocess.check_output(['docker','inspect',state['name']]))[0]
        self.assertEqual(info['Config']['Labels']['wildcard.benchmark.owner'],state['owner'])
        first=sweeps.Engine(state_path,timeout_seconds=10)
        self.addCleanup(first.close)
        second=sweeps.Engine(state_path,timeout_seconds=10)
        self.addCleanup(second.close)
        other_id='wildcard-isolation-'+second.database
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            owned=pool.submit(first.query,'SELECT sleep(3)',query_id=first.prefix+'probe')
            other=pool.submit(second.query,'SELECT sleep(3)',query_id=other_id)
            deadline=time.monotonic()+2
            while True:
                rows=first.query('SELECT current_database,query_id FROM system.processes WHERE query_id IN ('
                                 +sweeps.literal(first.prefix+'probe')+','+sweeps.literal(other_id)+')',database='default')
                if len(rows)==2 or time.monotonic()>=deadline:break
                time.sleep(.02)
            self.assertCountEqual(rows,[[first.database,first.prefix+'probe'],[second.database,other_id]])
            cancel_native_queries(first)
            with self.assertRaisesRegex(RuntimeError,'(?i)cancel'):owned.result()
            self.assertEqual(other.result(),[[0]],'cancellation disturbed another database')


class ObserverIdentityTests(unittest.TestCase):
    def test_nonlogged_observers_use_separate_ids_and_preserve_explicit_ids(self):
        from unittest.mock import MagicMock
        from urllib.parse import parse_qs,urlparse
        engine=sweeps.Engine.__new__(sweeps.Engine)
        engine.state={'url':'http://127.0.0.1','token':'fixture'}
        engine.settings={};engine.database='wc_benchmark_'+'a'*32
        engine.prefix=engine.database+'_';engine.timeout_seconds=10
        engine.opener=MagicMock()
        engine.opener.open.return_value.__enter__.return_value.read.return_value=b'{"data":[]}'
        engine.query('SELECT 1',logging=False)
        engine.query('SELECT 1')
        engine.query('SELECT 1',query_id='explicit',logging=False)
        ids=[parse_qs(urlparse(call.args[0].full_url).query)['query_id'][0] for call in engine.opener.open.call_args_list]
        self.assertTrue(ids[0].startswith('wc-observer-'))
        self.assertTrue(ids[1].startswith(engine.prefix))
        self.assertEqual(ids[2],'explicit')

if __name__=='__main__':unittest.main()
