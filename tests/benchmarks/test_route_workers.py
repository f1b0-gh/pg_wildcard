# SPDX-License-Identifier: Apache-2.0
import unittest
from unittest.mock import Mock
from pathlib import Path
from route_workers import DirectWorker, PgWorker


class WorkerCleanup(unittest.TestCase):
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
