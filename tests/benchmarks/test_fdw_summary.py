# SPDX-License-Identifier: Apache-2.0
import csv
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class FdwSummary(unittest.TestCase):
    def summarize(self, observations):
        with tempfile.TemporaryDirectory() as work:
            root=Path(work);run=root/'run';run.mkdir()
            (run/'settings.json').write_text(json.dumps({'arguments':{'timing_off':True}}))
            records=[]
            for values in observations:
                row=dict(workload='narrow',policy='in_band',batch_bytes=1024,concurrency=1,
                         route='direct',rows=10,wall_ms=1000,
                         query_log=[dict(query_duration_ms=900,read_rows=10,read_bytes=80,ProfileEvents=values.get('events',{}))])
                row.update({k:v for k,v in values.items() if k!='events'});records.append(row)
            (run/'raw.jsonl').write_text(''.join(json.dumps(r)+'\n' for r in records))
            subprocess.run([sys.executable,str(Path(__file__).with_name('summarize_fdw.py')),str(root)],check=True,capture_output=True)
            return json.loads((root/'summary.json').read_text())[0],next(csv.DictReader(io.StringIO((root/'summary.csv').read_text())))

    def test_unavailable_counters_and_resources_stay_missing(self):
        row,csvrow=self.summarize([{}])
        for name in ('files_read','parquet_rows_filtered','peak_batch_bytes','peak_rss_mib','max_threads'):
            self.assertIsNone(row[name],name)
            self.assertEqual(csvrow[name],'',name)
            self.assertEqual(row[name+'_samples'],0,name)

    def test_partial_observations_preserve_measured_zero_and_coverage(self):
        row,_=self.summarize([
            {},
            {'events':{'EngineFileLikeReadFiles':0},'process_peak':{'rss_kib':0,'threads':2},'peak_individual_batch_bytes':0},
            {'events':{'EngineFileLikeReadFiles':8},'process_peak':{'rss_kib':1024,'threads':3},'peak_individual_batch_bytes':128}])
        self.assertEqual(row['samples'],3)
        self.assertEqual(row['files_read'],4)
        self.assertEqual(row['files_read_samples'],2)
        self.assertIsNone(row['parquet_rows_filtered'])
        self.assertEqual(row['peak_rss_mib'],1)
        self.assertEqual(row['peak_batch_bytes'],128)
        self.assertEqual(row['max_threads'],3)
        for name in ('peak_rss_mib','peak_batch_bytes','max_threads'):
            self.assertEqual(row[name+'_samples'],2)


if __name__=='__main__':unittest.main()
