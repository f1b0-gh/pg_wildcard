# SPDX-License-Identifier: Apache-2.0
import unittest
import tempfile
from unittest.mock import Mock, patch
from pathlib import Path
from route_workers import DirectWorker, PgWorker


class WorkerCleanup(unittest.TestCase):
    def test_statement_timeout_is_typed_without_exposing_worker_stderr(self):
        with tempfile.TemporaryDirectory() as directory:
            log=Path(directory)/'worker.log'
            log.write_text('private diagnostic\nERROR:  canceling statement due to statement timeout\n')
            worker=PgWorker.__new__(PgWorker)
            worker.log=log.open('ab');worker.process=Mock()
            worker.process.stdout.fileno.return_value=42
            worker.pending=bytearray();worker.remote='/workspace/result'
            try:
                with patch('route_workers.select.select',return_value=([worker.process.stdout],[],[])), patch('route_workers.os.read',return_value=b''):
                    with self.assertRaisesRegex(TimeoutError,'^PostgreSQL statement timeout$'):
                        worker.execute('SELECT 1')
            finally:worker.log.close()

    def test_other_psql_errors_remain_fatal_and_private(self):
        with tempfile.TemporaryDirectory() as directory:
            log=Path(directory)/'worker.log';log.write_text('ERROR:  private unexpected failure\n')
            worker=PgWorker.__new__(PgWorker);worker.log=log.open('ab');worker.process=Mock()
            worker.process.stdout.fileno.return_value=42
            worker.pending=bytearray();worker.remote='/workspace/result'
            try:
                with patch('route_workers.select.select',return_value=([worker.process.stdout],[],[])), patch('route_workers.os.read',return_value=b''):
                    with self.assertRaisesRegex(RuntimeError,'^psql failed; inspect private worker log$'):
                        worker.execute('SELECT 1')
            finally:worker.log.close()

    def test_closed_worker_pipe_does_not_skip_reaping_or_log_cleanup(self):
        for kind in (DirectWorker,PgWorker):
            with self.subTest(worker=kind.__name__):
                worker=kind.__new__(kind);worker.process=Mock();worker.log=Mock()
                worker.process.stdin.write.side_effect=BrokenPipeError()
                worker.process.stdin.close.side_effect=BrokenPipeError()
                worker.process.wait.return_value=1
                worker.close()
                worker.process.wait.assert_called_once()
                worker.process.stdout.close.assert_called_once()
                worker.log.close.assert_called_once()

if __name__=='__main__':unittest.main()
