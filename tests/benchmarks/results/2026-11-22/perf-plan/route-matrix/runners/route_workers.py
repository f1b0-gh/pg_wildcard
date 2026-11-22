# SPDX-License-Identifier: Apache-2.0
"""Persistent worker protocols. Credentials never enter recorded commands."""
import json
import os
from pathlib import Path
import select
import subprocess
import time
import uuid
from comparison import MAX_BYTES, copy_pairs


class DirectWorker:
    def __init__(self, executable, state, log, extra_args=()):
        env = os.environ.copy()
        env.update(ANTALYA_TEST_URL=state['url'], ANTALYA_TEST_TOKEN=state['token'])
        self.log = open(log, 'wb'); os.chmod(log, 0o600)
        start = time.monotonic()
        self.process = subprocess.Popen([str(executable), *extra_args], env=env, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=self.log)
        try: self.ready = self.read(70)
        except BaseException: self.close(); raise
        self.setup_ms = (time.monotonic() - start) * 1000

    def read(self, timeout):
        deadline = time.monotonic() + timeout
        data = bytearray()
        while True:
            if time.monotonic() >= deadline or not select.select([self.process.stdout], [], [], max(0, deadline-time.monotonic()))[0]:
                raise TimeoutError('direct worker deadline')
            chunk = os.read(self.process.stdout.fileno(), 65536)
            if not chunk: raise RuntimeError('direct worker exited')
            data.extend(chunk)
            if len(data) > MAX_BYTES: raise ValueError('direct response byte bound')
            if data.endswith(b'\n'): return json.loads(data)

    def request(self, database, queries):
        data = json.dumps({'database': database, 'queries': queries}).encode() + b'\n'
        if len(data) > 8 << 20: raise ValueError('direct command bound')
        start = time.monotonic()
        self.process.stdin.write(data); self.process.stdin.flush()
        result = self.read(75)
        result['worker_roundtrip_ms'] = (time.monotonic()-start)*1000
        if 'error' in result: raise RuntimeError(result['error'])
        return result

    def close(self):
        if getattr(self, 'process', None):
            try: self.process.stdin.close()
            except (BrokenPipeError,OSError): pass
            try: self.process.wait(timeout=5)
            except subprocess.TimeoutExpired: self.process.kill(); self.process.wait(timeout=5)
            self.process.stdout.close()
        if getattr(self, 'log', None): self.log.close()


class PgWorker:
    """A persistent psql TCP session with bounded, file-backed result consumption.

    pg_container must be an explicitly owned fixture; shared_root is the existing
    /workspace bind. Server statement_timeout and client deadlines both apply.
    """
    def __init__(self, pg_container, shared_root, log):
        self.container = pg_container
        self.tag = 'wc_perf_'+uuid.uuid4().hex
        self.root = Path(shared_root).resolve()
        self.log = open(log, 'wb'); os.chmod(log, 0o600)
        self.file = self.root / '.local/perf/execution' / ('pg-output-'+uuid.uuid4().hex)
        self.remote = '/workspace/' + str(self.file.relative_to(self.root))
        self.process = subprocess.Popen(['docker','exec','-i',pg_container,'timeout','--signal=TERM','--kill-after=5s','1800s',
            'psql','-h','127.0.0.1','-U','postgres','-d','postgres','-XAtq','-v','ON_ERROR_STOP=1'],
            stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.log)
        self.pending = bytearray()
        start=time.monotonic()
        try:
            self.execute(f"SET application_name='{self.tag}'; SET statement_timeout='60s'; SELECT pg_backend_pid();")
            self.pid=int(self.file.read_text().strip())
        except BaseException: self.close(); raise
        self.setup_ms=(time.monotonic()-start)*1000

    def execute(self, sql):
        marker='done_'+uuid.uuid4().hex
        command=f'\\o {self.remote}\n{sql}\n\\o\n\\echo {marker}\n'.encode()
        if len(command)>16<<20: raise ValueError('PG command bound')
        start=time.monotonic()
        self.process.stdin.write(command); self.process.stdin.flush()
        deadline=start+75
        while True:
            if b'\n' in self.pending:
                line, _, rest=self.pending.partition(b'\n');self.pending=bytearray(rest)
                if line.decode().strip()==marker: break
                continue
            if time.monotonic()>=deadline or not select.select([self.process.stdout],[],[],max(0,deadline-time.monotonic()))[0]:
                raise TimeoutError('PG worker deadline')
            chunk=os.read(self.process.stdout.fileno(),65536)
            if not chunk:
                # Classify the observed server timeout without exposing private SQL
                # or credentials from stderr. Other psql failures remain fatal.
                with open(self.log.name, 'rb') as log:
                    log.seek(0, os.SEEK_END)
                    log.seek(max(0, log.tell()-65536))
                    if b'ERROR:  canceling statement due to statement timeout' in log.read().splitlines():
                        raise TimeoutError('PostgreSQL statement timeout')
                raise RuntimeError('psql failed; inspect private worker log')
            self.pending.extend(chunk)
            if len(self.pending)>MAX_BYTES: raise ValueError('PG protocol output bound')
        elapsed=(time.monotonic()-start)*1000
        if self.file.stat().st_size>MAX_BYTES: raise ValueError('PG result byte bound')
        return elapsed

    def request(self, sql, backend=False):
        if backend:
            elapsed=self.execute('EXPLAIN(ANALYZE,VERBOSE,TIMING OFF,FORMAT JSON) '+sql+';')
            plan=json.loads(self.file.read_bytes())[0]
            return {'request_ms':elapsed,'executor_ms':plan['Execution Time'],'planning_ms':plan['Planning Time'],'plan':plan,'backend_pid':self.pid}
        elapsed=self.execute('COPY ('+sql+') TO STDOUT (FORMAT binary);')
        # Result file is fully written/closed before the completion marker.
        # Decode/hash only after the request-to-consumption timer has stopped.
        raw=self.file.read_bytes()
        return {'request_ms':elapsed,'pairs':copy_pairs(raw),'copy_bytes':len(raw),'backend_pid':self.pid}

    def close(self):
        if getattr(self,'process',None):
            try: self.process.stdin.write(b'\\q\n'); self.process.stdin.flush()
            except (BrokenPipeError,OSError): pass
            try: self.process.stdin.close()
            except (BrokenPipeError,OSError): pass
            try:self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                # Terminate only the backend whose unique session name we set.
                if getattr(self,'pid',None):
                    subprocess.run(['docker','exec',self.container,'psql','-U','postgres','-XAtq','-c',
                        f"SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid={self.pid} AND application_name='{self.tag}'"],
                        stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10,check=True)
                self.process.kill();self.process.wait(timeout=5)
            self.process.stdout.close()
        if getattr(self,'log',None):self.log.close()
        if getattr(self,'file',None):self.file.unlink(missing_ok=True)
