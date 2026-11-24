# SPDX-License-Identifier: Apache-2.0
import threading
import time
import unittest
from unittest.mock import patch
from native_parallel import run


class Engine:
    def __init__(self, *, candidates=10, fail=False):
        self.prefix='wc_benchmark_'+'0'*32+'_'
        self.settings={'max_threads':4,'max_memory_usage':4_000_000_000}
        self.timeout_seconds=60
        self.trace={'lock':threading.Lock(),'active':0,'peak':0,'ops':[],
                    'candidates':candidates,'fail':fail,'killed':False}

    def query(self, sql, **kwargs):
        t=self.trace
        with t['lock']:t['ops'].append(sql)
        if sql.startswith('KILL'):t['killed']=True;return []
        if sql.startswith('DROP'):
            assert t['active']==0, 'drop before workers joined'
            return []
        if sql.startswith('SELECT count()'):return [[t['candidates'],t['candidates']*10]]
        if sql.startswith('CREATE'):return []
        with t['lock']:
            t['active']+=1;t['peak']=max(t['peak'],t['active'])
        try:
            if 'barrier' in t:
                t['barrier'].wait(timeout=10)
            else:
                time.sleep(.01)
            if t['fail']:raise RuntimeError('controlled query failure')
            return [[7, int(sql.removeprefix('q')), 'raw']]
        finally:
            with t['lock']:t['active']-=1


class ParallelTests(unittest.TestCase):
    def test_acquisition_uses_remaining_budget_then_cancels_and_drops(self):
        now=[0.0];budgets=[]
        class SlowAcquisition(Engine):
            def query(self,sql,**kwargs):
                if sql.startswith('SELECT count()'):now[0]=59.0
                if sql.startswith('CREATE'):
                    budgets.append(self.timeout_seconds)
                    now[0]=61.0
                return super().query(sql,**kwargs)
        e=SlowAcquisition()
        with patch('native_parallel.time.monotonic',side_effect=lambda:now[0]):
            with self.assertRaises(TimeoutError):
                run(e,{'queries':['q0']},source_rows=1000,
                    staged={'table':'wc_staged','candidate_select':'SELECT id,v FROM bench','queries':['q1']})
        self.assertEqual(budgets,[1])
        self.assertTrue(e.trace['killed'])
        self.assertTrue(e.trace['ops'][-1].startswith('DROP TABLE'))
        self.assertEqual(e.trace['active'],0)

    def test_bounded_workers_keep_all_batches_and_duplicate_rows(self):
        e=Engine()
        # Require real overlap without relying on a short sleep or host load.
        e.trace['barrier']=threading.Barrier(4)
        r=run(e,{'queries':['q0']*8},source_rows=1000)
        self.assertEqual(r['pairs'],[[7,0,'raw']]*8)
        self.assertGreater(e.trace['peak'],1)
        self.assertLessEqual(e.trace['peak'],4)
        self.assertEqual(e.settings['max_threads'],4)

    def test_dense_preflight_falls_back_before_materializing(self):
        e=Engine(candidates=1000)
        r=run(e,{'queries':['q0','q1']},source_rows=1000,
              staged={'table':'wc_staged','candidate_select':'SELECT id,v FROM bench','queries':['q9']})
        self.assertCountEqual(r['pairs'],[[7,0,'raw'],[7,1,'raw']])
        self.assertTrue(r['staging_refused'])
        self.assertFalse(any(q.startswith('CREATE') for q in e.trace['ops']))

    def test_staging_failure_cancels_joins_and_drops_owned_table(self):
        e=Engine(fail=True)
        with self.assertRaisesRegex(RuntimeError,'controlled query failure'):
            run(e,{'queries':['q0']},source_rows=1000,
                staged={'table':'wc_staged','candidate_select':'SELECT id,v FROM bench','queries':['q1','q2']})
        self.assertTrue(e.trace['killed'])
        self.assertTrue(e.trace['ops'][-1].startswith('DROP TABLE'))
        self.assertEqual(e.trace['active'],0)

    def test_rejects_unbounded_or_foreign_staging_spec(self):
        with self.assertRaises(ValueError):run(Engine(),{'queries':['q0']*9},source_rows=1000)
        with self.assertRaises(ValueError):
            run(Engine(),{'queries':['q0']},source_rows=1000,
                staged={'table':'other_table','candidate_select':'SELECT id,v FROM bench','queries':['q0']})

if __name__=='__main__':unittest.main()
