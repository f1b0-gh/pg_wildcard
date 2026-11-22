# SPDX-License-Identifier: Apache-2.0
import json
import struct
import unittest
from unittest.mock import Mock, patch
from compare_routes import native_run, elastic_run
from route_workers import PgWorker


class RouteDeadline(unittest.TestCase):
    def test_late_last_native_query_is_not_a_successful_sample(self):
        engine=Mock(prefix='owned_');engine.query.return_value=[]
        with patch('compare_routes.time.monotonic',side_effect=[0,0,61]):
            with self.assertRaises(TimeoutError):native_run(engine,['SELECT 1'])

    def test_late_last_elastic_batch_is_not_a_successful_sample(self):
        client=Mock();client.request.return_value={'responses':[{'timed_out':False,'_shards':{'failed':0},'hits':{'total':{'value':0,'relation':'eq'},'hits':[]},'took':61000}]}
        spec={'patterns':[{'id':1,'view':'raw','elastic_wildcard':'*none*'}]}
        with patch('compare_routes.time.monotonic',side_effect=[0,0,61]):
            with self.assertRaises(TimeoutError):elastic_run(client,'owned',spec)

    def test_late_complete_pg_backend_and_copy_are_not_successful_samples(self):
        for backend in (False, True):
            with self.subTest(backend=backend):
                worker=PgWorker.__new__(PgWorker)
                worker.pid=123;worker.file=Mock();worker.execute=Mock(return_value=61000)
                worker.file.read_bytes.return_value=(
                    json.dumps([{'Execution Time':59000,'Planning Time':1,'Plan':{}}]).encode()
                    if backend else b'PGCOPY\n\xff\r\n\0'+struct.pack('!iih',0,0,-1))
                with self.assertRaisesRegex(TimeoutError,'PG sweep completed after deadline'):
                    worker.request('SELECT 1 WHERE false',backend=backend)

if __name__=='__main__':unittest.main()
