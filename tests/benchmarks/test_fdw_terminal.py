# SPDX-License-Identifier: Apache-2.0
import unittest
from fdw_terminal import validate_file_reads, index_load_sql

class TerminalEvidenceTests(unittest.TestCase):
    def test_index_load_uses_client_files_in_one_transaction(self):
        sql=index_load_sql('/workspace/private/index')
        self.assertTrue(sql.startswith('BEGIN;\n'))
        self.assertTrue(sql.endswith('COMMIT;'))
        self.assertEqual(sql.count('\\copy '),3)
        self.assertNotIn('COPY ',sql)
        for path in ["/workspace/quote'",'/workspace/newline\n','/workspace/back\\slash']:
            self.assertRaises(ValueError,index_load_sql,path)
        self.assertIn("FROM '/workspace/private/index/files.csv' CSV\n",sql)

    def test_file_pruning_requires_actual_native_file_counters(self):
        for workload,count in [('lake_indexed',1),('lake_full',8)]:
            record=dict(workload=workload,profile={'profile_events':{'EngineFileLikeReadFiles':count,'ParquetRowsFilterExpression':count*125000}})
            self.assertEqual(validate_file_reads(record),count)
            for wrong in [0,7,9]:
                record['profile']['profile_events']['EngineFileLikeReadFiles']=wrong
                self.assertRaises(ValueError,validate_file_reads,record)
        self.assertRaises(ValueError,validate_file_reads,dict(workload='lake_indexed',profile={'profile_events':{}}))
        self.assertRaises(ValueError,validate_file_reads,dict(workload='lake_indexed',profile={'profile_events':{'EngineFileLikeReadFiles':1,'ParquetRowsFilterExpression':1000000}}))

if __name__=='__main__':unittest.main()
