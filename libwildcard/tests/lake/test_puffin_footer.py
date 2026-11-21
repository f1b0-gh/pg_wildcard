# SPDX-License-Identifier: Apache-2.0
import json
import unittest
from puffin_footer import read_footer


def fixture(**changes):
    blob = {"type": "wildcard-ngram-v1", "fields": [1], "snapshot-id": 42,
            "sequence-number": 7, "offset": 4, "length": 2}
    blob.update(changes)
    footer = json.dumps({"blobs": [blob]}).encode()
    return b"PFA1abPFA1" + footer + len(footer).to_bytes(4, "little", signed=True) + b"\0" * 4 + b"PFA1"


class FooterOracle(unittest.TestCase):
    def test_metadata_fields_and_extension_are_preserved(self):
        footer = read_footer(fixture())
        self.assertEqual(footer.blobs[0].snapshot_id, 42)
        self.assertEqual(footer.blobs[0].fields, [1])

    def test_wrong_discriminator_and_blob_bounds_are_rejected(self):
        for changes in [{"type": "deletion-vector-v1"}, {"offset": 3}, {"length": -1}, {"length": 1000}]:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                read_footer(fixture(**changes))

    def test_bad_envelopes_are_rejected(self):
        valid = fixture()
        for broken in [valid[:10], valid[:-1], valid[:-12] + b"\xff"*4 + valid[-8:],
                       valid[:-8] + b"\1\0\0\0" + valid[-4:]]:
            with self.assertRaises(ValueError):
                read_footer(broken)


if __name__ == "__main__":
    unittest.main()
