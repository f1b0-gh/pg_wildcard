# SPDX-License-Identifier: Apache-2.0
import struct
import unittest
from fdw_values import verify_copy, expected_row


def payload(rows):
    out=bytearray(b'PGCOPY\n\xff\r\n\0'+b'\0'*8)
    for row in rows:
        out+=struct.pack('!h',len(row))
        for value in row:
            out+=struct.pack('!i',-1 if value is None else len(value))
            if value is not None:out+=value
    return bytes(out+b'\xff\xff')


class FdwValuesTests(unittest.TestCase):
    def test_empty_selective_control_rejects_truncation_and_any_row(self):
        self.assertEqual(verify_copy(payload([]),'selective',0)['rows'],0)
        for raw in [payload([])[:-1], payload([])+b'x',
                    payload([expected_row('selective',0)])]:
            with self.assertRaises(ValueError):verify_copy(raw,'selective',0)


    def test_selective_fixture_checks_sparse_ids_and_every_value(self):
        rows=[(struct.pack('!q',i*1000),b'evil.test/powershell',b'x'*16) for i in range(3)]
        self.assertEqual(verify_copy(payload(rows[::-1]),'selective',3)['rows'],3)
        for identifier in [1,999,3000,-1000]:
            bad=[(struct.pack('!q',identifier),rows[0][1],rows[0][2])]+rows[1:]
            with self.assertRaises(ValueError):verify_copy(payload(bad),'selective',3)
        for bad in [rows[:2], [rows[0],rows[0],rows[2]],
                    [rows[0],(rows[1][0],b'benign.test/normal',rows[1][2]),rows[2]]]:
            with self.assertRaises(ValueError):verify_copy(payload(bad),'selective',3)

    def test_exact_values_and_complete_id_set_ignore_arrival_order(self):
        for workload in ('narrow','smalltext','wide1k','broad_recheck'):
            rows=[expected_row(workload,i) for i in range(5)]
            raw=payload(rows[::-1])
            result=verify_copy(raw,workload,5)
            self.assertEqual(result['rows'],5)
            self.assertEqual(result['copy_bytes'],len(raw))

    def test_duplicate_missing_wrong_value_and_null_are_rejected(self):
        rows=[expected_row('smalltext',i) for i in range(3)]
        for wrong in [rows[:2],rows+[rows[0]],[rows[0],rows[0],rows[2]],
                      [rows[0],(rows[1][0],b'wrong',rows[1][2]),rows[2]],
                      [rows[0],(rows[1][0],None,rows[1][2]),rows[2]]]:
            with self.assertRaises(ValueError):verify_copy(payload(wrong),'smalltext',3)

    def test_truncated_or_extended_payload_and_invalid_lengths_rejected(self):
        raw=payload([expected_row('narrow',0)])
        for value in [raw[:-1],raw+b'x',raw[:19]+b'\0\1\xff\xff\xff\xfe'+raw[25:],raw[:19]+b'\0\2'+raw[21:]]:
            with self.assertRaises(ValueError):verify_copy(value,'narrow',1)
        with self.assertRaises(ValueError):verify_copy(raw,'narrow',1000001)

if __name__=='__main__':unittest.main()
