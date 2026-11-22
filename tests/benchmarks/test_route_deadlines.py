# SPDX-License-Identifier: Apache-2.0
import unittest
from unittest.mock import Mock, patch
from compare_routes import native_run, elastic_run


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

if __name__=='__main__':unittest.main()
