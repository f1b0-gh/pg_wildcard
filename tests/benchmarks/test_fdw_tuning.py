# SPDX-License-Identifier: Apache-2.0
import unittest

from fdw_tuning import schedule, summarize, selected_sql


class TuningTests(unittest.TestCase):
    def test_correlation_removes_only_the_client_arrow_format_suffix(self):
        self.assertEqual(selected_sql('SELECT `id` FROM events\nFORMAT ArrowStream'),
                         'SELECT `id` FROM events')
        sql="SELECT 'FORMAT ArrowStream' FROM events"
        self.assertEqual(selected_sql(sql),sql)
        self.assertEqual(selected_sql('SELECT `id` FROM events\n'),'SELECT `id` FROM events')
        self.assertEqual(selected_sql('SELECT `id` FROM events\nFORMAT ArrowStream\n'),'SELECT `id` FROM events')
        self.assertNotEqual(selected_sql('SELECT 2\nFORMAT ArrowStream'),'SELECT 1')

    def test_native_control_uses_the_same_uncompressed_arrow_profile_as_client(self):
        import io
        from types import SimpleNamespace
        from urllib.parse import parse_qs,urlparse
        from fdw_tuning import native_request
        requests=[]
        def open_request(request,timeout):
            requests.append(request)
            return io.BytesIO(b'arrow-body')
        engine=SimpleNamespace(database='fixture',state={'url':'http://fixture.invalid','token':'test'},
                               opener=SimpleNamespace(open=open_request))
        result,raw=native_request(engine,'SELECT 1',4096)
        settings=parse_qs(urlparse(requests[0].full_url).query)
        self.assertEqual(settings['output_format_arrow_compression_method'],['none'])
        self.assertEqual(settings['enable_http_compression'],['0'])
        self.assertEqual(settings['max_block_size'],['4096'])
        self.assertEqual(raw,b'arrow-body')
        self.assertEqual(result['http_body_bytes'],len(raw))

    def test_each_round_covers_all_controls_and_rotates_first_case(self):
        configs=[('in_band',n) for n in (1024,4096,16384)]
        runs=list(schedule(configs,('native','direct','pg_backend','pg_copy'),5))
        self.assertEqual(len(runs),72)
        for repeat in range(-1,5):
            cohort=[(policy,block,route) for n,policy,block,route in runs if n==repeat]
            self.assertEqual(len(set(cohort)),12)
        first=[next((p,b,r) for n,p,b,r in runs if n==repeat) for repeat in range(5)]
        self.assertEqual(len(set(first)),5)

    def test_summary_excludes_warmup_keeps_refusals_and_separates_routes(self):
        common=dict(workload='narrow',policy='in_band',block_size=1024)
        records=[dict(common,route='direct',repeat=-1,request_ms=9999,status='success'),
                 dict(common,route='direct',repeat=0,request_ms=10,status='success'),
                 dict(common,route='direct',repeat=1,request_ms=20,status='success'),
                 dict(common,route='direct',repeat=2,status='resource_refusal'),
                 dict(common,route='pg_copy',repeat=0,request_ms=50,status='success')]
        result=summarize(records)
        direct=next(r for r in result if r['route']=='direct')
        self.assertEqual(direct['median_ms'],15)
        self.assertEqual(direct['samples'],2)
        self.assertEqual(direct['refusals'],1)
        self.assertEqual(next(r for r in result if r['route']=='pg_copy')['median_ms'],50)

    def test_missing_or_failed_samples_cannot_be_reported_as_success(self):
        record=dict(workload='narrow',policy='in_band',block_size=1024,
                    route='direct',repeat=0,status='error')
        self.assertRaises(ValueError,summarize,[record])
        record['status']='success'
        self.assertRaises(ValueError,summarize,[record])


if __name__=='__main__':unittest.main()
