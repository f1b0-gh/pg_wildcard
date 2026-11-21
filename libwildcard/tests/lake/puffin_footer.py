# SPDX-License-Identifier: Apache-2.0
"""PyIceberg metadata models extended with exactly our sidecar blob type.

PyIceberg 0.10's Puffin reader is specific to deletion vectors. Its generic
metadata fields are useful here, but its Literal discriminator must explicitly
admit the extension. This validates the footer/envelope, not n-gram payloads.
See https://iceberg.apache.org/puffin-spec/ (blob type is an extensible string).
"""
from typing import Literal
from pyiceberg.table.puffin import Footer, PuffinBlobMetadata


class WildcardBlob(PuffinBlobMetadata):
    type: Literal["wildcard-ngram-v1"]


class WildcardFooter(Footer):
    blobs: list[WildcardBlob]


def read_footer(raw: bytes) -> WildcardFooter:
    if len(raw) < 20 or raw[:4] != b"PFA1" or raw[-4:] != b"PFA1":
        raise ValueError("invalid Puffin magic or truncated envelope")
    if raw[-8:-4] != b"\0\0\0\0":
        raise ValueError("this oracle accepts uncompressed footers only")
    size = int.from_bytes(raw[-12:-8], "little", signed=True)
    if size < 0 or size > len(raw) - 20:
        raise ValueError("invalid Puffin footer length")
    start = len(raw) - size - 12
    if raw[start-4:start] != b"PFA1":
        raise ValueError("missing footer magic")
    footer = WildcardFooter.model_validate_json(raw[start:-12])
    end = 4
    for blob in sorted(footer.blobs, key=lambda b: b.offset):
        if blob.offset < end or blob.length < 0 or blob.offset + blob.length > start - 4:
            raise ValueError("invalid blob bounds or overlapping blobs")
        end = blob.offset + blob.length
    return footer
