#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Linux /proc sampler; stdout contains process-local RSS, HWM and threads."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time


def status(pid):
    try:
        fields={line.split(':',1)[0]:line.split(':',1)[1].strip() for line in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in line}
        return {'pid':int(pid),'rss_kib':int(fields.get('VmRSS','0 kB').split()[0]),'hwm_kib':int(fields.get('VmHWM','0 kB').split()[0]),'threads':int(fields.get('Threads','0'))}
    except (OSError,ValueError):return None

class HeartbeatLease:
    """A marker must keep changing; monotonic time bounds orphan lifetime."""
    def __init__(self, marker, timeout=3.0, clock=time.monotonic):
        self.marker = Path(marker)
        self.timeout = timeout
        self.clock = clock
        self.modified = None
        self.changed_at = clock()

    def alive(self):
        try:
            modified = self.marker.stat().st_mtime_ns
        except FileNotFoundError:
            return False
        now = self.clock()
        if modified != self.modified:
            self.modified = modified
            self.changed_at = now
        return now - self.changed_at < self.timeout


class SamplerProcess:
    """Own marker, heartbeat, child and signal handlers as one lifetime.

    The child's lease expires even if SIGKILL prevents host cleanup. Updating
    uses utime rather than touch, so shutdown cannot recreate a removed marker.
    """
    def __init__(self, command, marker, heartbeat_interval=.25, shutdown_timeout=5):
        self.command = command
        self.marker = Path(marker)
        self.heartbeat_interval = heartbeat_interval
        self.shutdown_timeout = shutdown_timeout
        self.process = None
        self.heartbeat = None
        self.stop = threading.Event()
        self.handlers = {}
        self.closed = False
        self.owns_marker = False

    def __enter__(self):
        try:
            self.marker.parent.mkdir(parents=True, exist_ok=True)
            self.marker.touch(exist_ok=False)
            self.owns_marker = True
            for signum in (signal.SIGINT, signal.SIGTERM):
                self.handlers[signum] = signal.getsignal(signum)
                signal.signal(signum, self._interrupted)
            self.process = subprocess.Popen(self.command, stdout=subprocess.PIPE, text=True)
            self.heartbeat = threading.Thread(target=self._beat, daemon=True)
            self.heartbeat.start()
            return self.process
        except BaseException:
            self.close()
            self._restore_handlers()
            raise

    def _beat(self):
        while not self.stop.wait(self.heartbeat_interval):
            try:
                os.utime(self.marker, None)
            except OSError:
                # Failure to refresh is fail-closed: the child lease expires.
                return

    def _interrupted(self, signum, _frame):
        # Stop before unwinding ThreadPoolExecutor, whose exit can wait for work.
        self.close()
        raise SystemExit(128 + signum)

    def close(self):
        if self.closed:
            return
        self.closed = True
        self.stop.set()
        if self.owns_marker:
            self.marker.unlink(missing_ok=True)
        if self.heartbeat is not None:
            self.heartbeat.join(timeout=1)
        if self.process is not None:
            try:
                self.process.wait(timeout=self.shutdown_timeout)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                try:
                    self.process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=1)

    def _restore_handlers(self):
        for signum, handler in self.handlers.items():
            signal.signal(signum, handler)
        self.handlers.clear()

    def __exit__(self, _kind, _value, _traceback):
        try:
            self.close()
        finally:
            self._restore_handlers()


def sample(marker, lease_seconds=3.0):
    lease = HeartbeatLease(marker, lease_seconds)
    # A dead host must not leave us blocked writing to an unread pipe before
    # checking the lease. Keep at most one snapshot pending; partial writes are
    # completed before starting another JSON line.
    fd = sys.stdout.fileno()
    os.set_blocking(fd, False)
    pending = b''
    while lease.alive():
        rows=[]
        if not pending:
            for p in Path('/proc').iterdir():
                if not p.name.isdigit():continue
                try:comm=(p/'comm').read_text().strip()
                except OSError:continue
                if comm=='postgres':
                    row=status(p.name)
                    if row:rows.append(row)
            pending = (json.dumps(rows) + '\n').encode()
        try:
            pending = pending[os.write(fd, pending):]
        except BlockingIOError:
            pass
        except BrokenPipeError:
            return
        time.sleep(.01)


if __name__=='__main__':
    if len(sys.argv) not in (2, 3):
        raise SystemExit('usage: process_sampler.py MARKER [LEASE_SECONDS]')
    lease_seconds = float(sys.argv[2]) if len(sys.argv) == 3 else 3.0
    if not 0 < lease_seconds <= 60:
        raise SystemExit('lease must be greater than zero and at most 60 seconds')
    sample(sys.argv[1], lease_seconds)
