#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Small Linux lifecycle regressions; no Docker, database or benchmark load."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from fdw_profile import POLICIES, WORKLOADS, csv_selection, persist_cohort, pg_literal, redact
from process_sampler import HeartbeatLease, SamplerProcess


SAMPLER = Path(__file__).with_name('process_sampler.py').resolve()


def running(pid):
    try:
        # An orphan can briefly remain a zombie until init reaps it; it has
        # already stopped sampling and consuming CPU.
        state = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0]
        return state != 'Z'
    except (OSError, IndexError):
        return False


class LeaseTests(unittest.TestCase):
    def test_expiry_uses_monotonic_elapsed_and_changes_refresh_it(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / 'lease'
            marker.touch()
            now = [0.0]
            lease = HeartbeatLease(marker, timeout=3, clock=lambda: now[0])
            self.assertTrue(lease.alive())
            now[0] = 2
            self.assertTrue(lease.alive())
            stamp = marker.stat().st_mtime_ns
            os.utime(marker, ns=(stamp + 1, stamp + 1))
            self.assertTrue(lease.alive())
            now[0] = 4.9
            self.assertTrue(lease.alive())
            now[0] = 5
            self.assertFalse(lease.alive())
            marker.unlink()
            self.assertFalse(lease.alive())

    def test_existing_marker_is_not_deleted(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / 'lease'
            marker.write_text('not ours')
            with self.assertRaises(FileExistsError):
                with SamplerProcess([sys.executable, '-c', 'pass'], marker):
                    pass
            self.assertEqual(marker.read_text(), 'not ours')

    def test_normal_and_exception_exit_stop_and_reap_child(self):
        for fail in (False, True):
            with self.subTest(fail=fail), tempfile.TemporaryDirectory() as directory:
                marker = Path(directory) / 'lease'
                child = None
                try:
                    with SamplerProcess([sys.executable, str(SAMPLER), str(marker)], marker) as child:
                        self.assertTrue(marker.exists())
                        if fail:
                            raise RuntimeError('primary failure')
                except RuntimeError as error:
                    self.assertTrue(fail)
                    self.assertEqual(str(error), 'primary failure')
                self.assertIsNotNone(child)
                self.assertIsNotNone(child.poll())
                self.assertFalse(marker.exists())
                child.stdout.close()

    def test_failed_child_start_removes_marker_and_restores_handlers(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / 'lease'
            before = signal.getsignal(signal.SIGTERM)
            with self.assertRaises(FileNotFoundError):
                with SamplerProcess([str(Path(directory) / 'absent')], marker):
                    pass
            self.assertFalse(marker.exists())
            self.assertEqual(signal.getsignal(signal.SIGTERM), before)

    def test_expired_lease_exits_even_when_stdout_is_full(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / 'lease'
            marker.touch()
            read_fd, write_fd = os.pipe()
            try:
                size = fcntl.fcntl(write_fd, fcntl.F_SETPIPE_SZ, 4096)
                os.write(write_fd, b'x' * size)
                child = subprocess.Popen([sys.executable, str(SAMPLER), str(marker), '.2'], stdout=write_fd)
                try:
                    self.assertEqual(child.wait(timeout=3), 0)
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait()
            finally:
                os.close(read_fd)
                os.close(write_fd)

    def test_signals_and_sigkill_bound_sampler_lifetime(self):
        helper = '''
import sys,time
sys.path.insert(0,sys.argv[1])
from process_sampler import SamplerProcess
with SamplerProcess([sys.executable,sys.argv[2],sys.argv[3],'.3'],sys.argv[3],heartbeat_interval=.03) as child:
    print(child.pid,child.stdout.fileno(),flush=True)
    while True:time.sleep(10)
'''
        for signum in (signal.SIGTERM, signal.SIGINT, signal.SIGKILL):
            with self.subTest(signal=signum), tempfile.TemporaryDirectory() as directory:
                marker = Path(directory) / 'lease'
                host = subprocess.Popen([sys.executable, '-c', helper, str(SAMPLER.parent), str(SAMPLER), str(marker)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                child_pid = None
                keeper = None
                try:
                    self.assertTrue(select.select([host.stdout], [], [], 3)[0], 'helper did not start')
                    child_pid, read_fd = map(int, host.stdout.readline().split())
                    # Keep the child's output pipe open across SIGKILL. Thus the
                    # test proves lease expiry, rather than BrokenPipe cleanup.
                    keeper = open(f'/proc/{host.pid}/fd/{read_fd}', 'rb', buffering=0)
                    self.assertTrue(select.select([keeper], [], [], 3)[0], 'sampler did not start')
                    self.assertTrue(keeper.readline(), 'sampler exited before emitting a sample')
                    host.send_signal(signum)
                    host.wait(timeout=4)
                    deadline = time.monotonic() + 3
                    while running(child_pid) and time.monotonic() < deadline:
                        time.sleep(.02)
                    self.assertFalse(running(child_pid), 'orphan sampler remained active')
                    self.assertEqual(marker.exists(), signum == signal.SIGKILL)
                finally:
                    if keeper is not None:
                        keeper.close()
                    if host.poll() is None:
                        host.kill()
                        host.wait()
                    if child_pid is not None and running(child_pid):
                        os.kill(child_pid, signal.SIGKILL)
                    host.stdout.close()
                    host.stderr.close()


class ProfileArtifactTests(unittest.TestCase):
    def test_quoted_options_are_escaped_and_error_redaction_covers_sql_spelling(self):
        value = "opaque'\\value"
        self.assertEqual(pg_literal(value), "E'opaque''\\\\value'")
        with self.assertRaises(ValueError):
            pg_literal('bad\0value')
        with patch.dict(os.environ, {'ANTALYA_TEST_TOKEN':value}):
            self.assertEqual(redact('option '+pg_literal(value)), 'option [redacted]')
            self.assertEqual(redact(value), '[redacted]')

    def test_policy_and_workload_selections_reject_unknown_sql_text(self):
        self.assertEqual(csv_selection(POLICIES)('flush_first,in_band'), 'flush_first,in_band')
        self.assertEqual(csv_selection(WORKLOADS)('wide1k,broad_recheck'), 'wide1k,broad_recheck')
        for value in ("flush_first'); SELECT 1; --", '', 'missing'):
            with self.assertRaises(argparse.ArgumentTypeError):
                csv_selection(POLICIES)(value)

    def test_control_failure_preserves_records_and_sanitizes_error(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {
            'ANTALYA_TEST_TOKEN':'secret-token', 'ANTALYA_TEST_URL':'http://private',
            'FDW_ENGINE_URL':'http://pg-private',
        }):
            out = Path(directory)
            cohort = [{'query_id':'q1','rows':3,'wall_ms':7}, {'error':'original worker failure'}]
            def fail():
                pending = list(out.glob('pending-cohort-*.json'))
                self.assertEqual(len(pending), 1)
                self.assertEqual(json.loads(pending[0].read_text())['records'], cohort)
                raise RuntimeError('secret-token http://private http://pg-private')
            with self.assertRaises(RuntimeError):
                persist_cohort(out, cohort, fail, lambda _: self.fail('lookup after failed flush'), 3)
            records = [json.loads(line) for line in (out/'raw.jsonl').read_text().splitlines()]
            self.assertEqual(len(records), 2)
            self.assertEqual(records[0]['wall_ms'], 7)
            self.assertEqual(records[1]['error'], 'original worker failure')
            self.assertEqual(records[0]['control_error'], '[redacted] [redacted] [redacted]')
            self.assertFalse(list(out.glob('pending-cohort-*.json')))

    def test_success_writes_each_record_once(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            cohort = [{'query_id':'q1','rows':3}, {'query_id':'q2','rows':3}]
            persist_cohort(out, cohort, lambda: None, lambda qid: [{'query_id':qid}], 3)
            records = [json.loads(line) for line in (out/'raw.jsonl').read_text().splitlines()]
            self.assertEqual(records, cohort)
            self.assertEqual(len(records), 2)
            self.assertFalse(any('error' in record for record in records))
            self.assertFalse(list(out.glob('pending-cohort-*.json')))

    def test_raw_write_failure_keeps_pending_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            (out/'raw.jsonl').mkdir()
            with self.assertRaises(IsADirectoryError):
                persist_cohort(out, [{'query_id':'q1','rows':3}], lambda: None, lambda _: [{}], 3)
            self.assertEqual(len(list(out.glob('pending-cohort-*.json'))), 1)


if __name__ == '__main__':
    unittest.main()
