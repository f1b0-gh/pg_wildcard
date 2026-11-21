# SPDX-License-Identifier: Apache-2.0
"""Isolated native backend for the existing differential harness."""
import atexit
import os
import json
import re
import secrets
import subprocess
import uuid
from urllib.request import Request, build_opener, ProxyHandler, HTTPRedirectHandler
from urllib.parse import urlencode

class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs):return None

class Result:
    def __init__(self,data):self.body=data
    def data(self):return self.body

class NativeSession:
    def __init__(self,container):
        owned_bearer_fixture = (
            container == os.environ.get('ANTALYA_TEST_CONTAINER')
            and re.fullmatch(r'antalya-rs-[a-f0-9]{12}', container)
        )
        if not (re.fullmatch(r'soc-http-test-[a-z0-9-]+-ch',container) or owned_bearer_fixture):
            raise ValueError('Only an explicitly selected owned fixture is supported')
        self.container=container;self.database='wc_diff_'+uuid.uuid4().hex;self.user=self.database
        self.password=secrets.token_hex(24)
        address=subprocess.check_output(['docker','inspect',container,'--format','{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}'],text=True).strip()
        self.url=f'http://{address}:8123/?'+urlencode({'database':self.database,'default_format':'JSONCompact','max_execution_time':'15','max_threads':'1','max_memory_usage':'100000000'})
        self.opener=build_opener(ProxyHandler({}),NoRedirect())
        atexit.register(self.close)
        self.admin(f"CREATE DATABASE {self.database}; CREATE USER {self.user} IDENTIFIED BY '{self.password}'; GRANT ALL ON {self.database}.* TO {self.user};")
        print('native backend:',self.admin('SELECT version()').strip(),flush=True)
    def admin(self,sql):
        return subprocess.check_output(['docker','exec','-i',self.container,'clickhouse-client','--multiquery'],input=sql,text=True)
    def close(self):
        self.admin(f'DROP DATABASE IF EXISTS {self.database}; DROP USER IF EXISTS {self.user};')
    def query(self,sql,format):
        if sql=='CREATE DATABASE IF NOT EXISTS d; USE d':return Result(b'')
        request=Request(self.url,data=sql.encode(),headers={'X-ClickHouse-User':self.user,'X-ClickHouse-Key':self.password})
        with self.opener.open(request,timeout=30) as response:
            body=response.read(16*1024*1024+1)
        if len(body)>16*1024*1024:raise ValueError('Differential response exceeds budget')
        return Result(body)
