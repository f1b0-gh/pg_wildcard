# SPDX-License-Identifier: Apache-2.0
import copy
import unittest

from fdw_correlation import bind_profiles


def sample(route, qid=None):
    result=dict(route=route,workload='narrow',repeat=0,policy='flush_first',
                block_size=4096,rows=10,status='success')
    if qid:result['query_id']=qid
    if route=='pg_backend':result['selected_sql']='SELECT id FROM fixture.events'
    return result


def logged(qid,sql='SELECT id FROM fixture.events\n'):
    return dict(query_id=qid,query=sql,query_duration_ms=2,read_rows=10,read_bytes=80,
                result_rows=10,result_bytes=80,memory_usage=1024,profile_events={},
                settings={'max_block_size':'4096','output_format_arrow_compression_method':'none'})


class CorrelationTests(unittest.TestCase):
    def test_copy_is_bound_to_unique_completion_and_exact_paired_sql(self):
        records=[sample('direct','direct-1'),sample('pg_backend','wildcard-1'),sample('pg_copy')]
        logs=[logged('direct-1'),logged('wildcard-1'),logged('wildcard-2')]
        result=bind_profiles(records,logs)
        self.assertEqual(result[2]['query_id'],'wildcard-2')
        self.assertEqual(result[0]['profile']['read_rows'],10)
        self.assertNotIn('query_id',records[2])

    def test_missing_duplicate_wrong_sql_or_wrong_settings_cannot_be_accepted(self):
        records=[sample('pg_backend','wildcard-1'),sample('pg_copy')]
        good=[logged('wildcard-1'),logged('wildcard-2')]
        cases=[good[:1],good+[good[1]],
               [good[0],logged('wildcard-2','SELECT id FROM wrong.events')]]
        for field,value in [('result_rows',9),('settings',{'max_block_size':'1024'})]:
            bad=copy.deepcopy(good);bad[1][field]=value;cases.append(bad)
        for logs in cases:
            with self.assertRaises(ValueError):bind_profiles(records,logs)

    def test_native_compression_difference_is_rejected(self):
        records=[sample('native','native-1')]
        logs=[logged('native-1')]
        logs[0]['settings']['output_format_arrow_compression_method']='lz4_frame'
        self.assertRaises(ValueError,bind_profiles,records,logs)


if __name__=='__main__':unittest.main()
