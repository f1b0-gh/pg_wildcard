# SPDX-License-Identifier: Apache-2.0
import os
import json
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import fdw_profile


class ProfileDeadlines(unittest.TestCase):
    def test_hung_direct_process_and_observer_are_reaped(self):
        self.assertTrue(hasattr(fdw_profile,'bounded_process'))
        with tempfile.TemporaryDirectory() as work:
            marker=Path(work)/'pid'
            code='import os,time; from pathlib import Path; Path('+repr(str(marker))+').write_text(str(os.getpid())); time.sleep(60)'
            started=time.monotonic()
            with self.assertRaisesRegex(TimeoutError,'deadline'):
                fdw_profile.bounded_process([sys.executable,'-c',code],data='',timeout=1,sample=True)
            self.assertLess(time.monotonic()-started,10)
            self.assertTrue(marker.exists())
            with self.assertRaises(ProcessLookupError):os.kill(int(marker.read_text()),0)
            self.assertFalse(any(t.name.startswith('fdw-profile-observer-') for t in threading.enumerate()))

    def test_pg_deadline_has_sql_limits_and_cleans_only_its_session(self):
        self.assertTrue(hasattr(fdw_profile,'pg_request'))
        with patch.object(fdw_profile,'bounded_process',side_effect=TimeoutError('deadline')) as run, \
             patch.object(fdw_profile,'terminate_pg_session') as cleanup:
            with self.assertRaises(TimeoutError):fdw_profile.pg_request('owned-pg','ALTER SERVER fixture OPTIONS (SET x \'y\')')
            command=run.call_args.args[0]
            tag=next(arg.removeprefix('PGAPPNAME=') for arg in command if arg.startswith('PGAPPNAME='))
            self.assertTrue(tag.startswith('wc_profile_'))
            self.assertIn('PGOPTIONS=-c statement_timeout=300000 -c lock_timeout=10000',command)
            self.assertIn('timeout',command)
            self.assertGreater(run.call_args.kwargs['timeout'],0)
            cleanup.assert_called_once_with('owned-pg',tag)

    def test_pg_cleanup_failure_is_fatal(self):
        self.assertTrue(hasattr(fdw_profile,'pg_request'))
        with patch.object(fdw_profile,'bounded_process',side_effect=TimeoutError('deadline')), \
             patch.object(fdw_profile,'terminate_pg_session',side_effect=RuntimeError('cleanup unavailable')):
            with self.assertRaisesRegex(RuntimeError,'cleanup'):
                fdw_profile.pg_request('owned-pg','SELECT 1')

    def test_failed_cohort_is_saved_and_stops_further_profiling(self):
        with tempfile.TemporaryDirectory() as work:
            out=Path(work)
            cohort=[{'error':'worker deadline; remote completion unconfirmed'}]
            with self.assertRaisesRegex(RuntimeError,'failed cohort'):
                fdw_profile.persist_cohort(out,cohort,lambda:None,lambda _:[],0)
            self.assertEqual(json.loads((out/'raw.jsonl').read_text()),cohort[0])


if __name__=='__main__':unittest.main()
