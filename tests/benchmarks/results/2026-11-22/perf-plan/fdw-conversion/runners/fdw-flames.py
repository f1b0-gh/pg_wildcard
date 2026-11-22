"""Paired real CPU profiles for the exact release binaries used in latency trials."""
import hashlib
import json
import os
import signal
import subprocess
import time
from pathlib import Path

root = Path('.local/perf/fdw/flames')
if root.exists():
    raise RuntimeError('fresh paired-profile directory required')
root.mkdir(parents=True)
binaries = {'baseline': Path('.local/perf-review/fdw/pg_wildcard-measured-release.so'),
            'prepared': Path('.local/perf/execution/pg-prepared.so')}
records = []


def install(label):
    temporary = '/usr/lib/postgresql/18/lib/pg_wildcard.so.wc-perf'
    subprocess.run(['docker', 'exec', 'wildcard-review-pg18', 'cp',
                    '/workspace/' + str(binaries[label]), temporary], check=True, timeout=10)
    subprocess.run(['docker', 'exec', 'wildcard-review-pg18', 'mv', temporary,
                    '/usr/lib/postgresql/18/lib/pg_wildcard.so'], check=True, timeout=10)


def terminate(_signum, _frame):
    raise KeyboardInterrupt('owned paired profile termination')


signal.signal(signal.SIGTERM, terminate)
try:
    for label in ('baseline', 'prepared'):
        if Path('.local/perf/execution/timing-active').exists():
            raise RuntimeError('another timing window is active')
        install(label)
        started = time.monotonic()
        command = ['python3', '.local/perf/execution/fdw-flames-one.py', '--label', label]
        with (root / (label + '.log')).open('w') as log:
            child = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            (root / 'process.json').write_text(json.dumps(dict(label=label, pid=child.pid, command=command)) + '\n')
            try:
                rc = child.wait(timeout=350)
            except BaseException:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
                raise
        records.append(dict(label=label, exit_status=rc, seconds=time.monotonic()-started,
                            binary_sha256=hashlib.sha256(binaries[label].read_bytes()).hexdigest()))
        (root / 'status.json').write_text(json.dumps(records, indent=2) + '\n')
        print('PROFILE', label, rc, flush=True)
        if rc:
            raise SystemExit('inspect profile failure before continuing')
finally:
    install('baseline')
