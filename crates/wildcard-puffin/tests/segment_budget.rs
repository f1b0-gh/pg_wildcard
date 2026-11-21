// SPDX-License-Identifier: Apache-2.0
use std::collections::BTreeMap;
use wildcard_puffin::{
    puffin::{Blob, Codec, PuffinWriter},
    segments::{Budget, MAX_ARTIFACTS, MAX_COMPRESSED_BYTES, MAX_PAYLOAD_BYTES},
};
fn file(size: usize, codec: Codec, footer: bool) -> Vec<u8> {
    let mut w = PuffinWriter::new();
    w.add(
        Blob {
            blob_type: wildcard_puffin::ngram::BLOB_TYPE.into(),
            fields: vec![1],
            snapshot_id: 1,
            sequence_number: 1,
            properties: BTreeMap::new(),
            data: vec![0; size],
        },
        codec,
    )
    .unwrap();
    w.finish(BTreeMap::new(), footer).unwrap()
}
#[test]
fn payload_boundary_and_atomic_failure() {
    let mut b = Budget::default();
    b.charge(&file(MAX_PAYLOAD_BYTES / 2, Codec::Zstd, false))
        .unwrap();
    b.charge(&file(MAX_PAYLOAD_BYTES / 2, Codec::Zstd, true))
        .unwrap();
    assert_eq!(b.payload_bytes(), MAX_PAYLOAD_BYTES);
    let before = b.clone();
    assert!(b.charge(&file(1, Codec::Zstd, false)).is_err());
    assert_eq!(b, before);
}
#[test]
fn compressed_boundary_and_corruption() {
    let prototype = file(0, Codec::None, false);
    let exact = file(MAX_COMPRESSED_BYTES - prototype.len(), Codec::None, false);
    let exact = file(
        MAX_COMPRESSED_BYTES - prototype.len() - (exact.len() - MAX_COMPRESSED_BYTES),
        Codec::None,
        false,
    );
    assert_eq!(exact.len(), MAX_COMPRESSED_BYTES);
    let mut b = Budget::default();
    b.charge(&exact).unwrap();
    let before = b.clone();
    assert!(b.charge(&prototype).is_err());
    assert_eq!(b, before);
    let mut clean = Budget::default();
    assert!(clean.charge(b"PFA1").is_err());
    assert_eq!(clean, Budget::default());
}
#[test]
fn artifact_limit_and_single_own_blob() {
    let f = file(0, Codec::None, false);
    let mut b = Budget::default();
    for _ in 0..MAX_ARTIFACTS {
        b.charge(&f).unwrap();
    }
    let before = b.clone();
    assert!(b.charge(&f).is_err());
    assert_eq!(b, before);
    let mut w = PuffinWriter::new();
    for _ in 0..2 {
        w.add(
            Blob {
                blob_type: wildcard_puffin::ngram::BLOB_TYPE.into(),
                fields: vec![1],
                snapshot_id: 1,
                sequence_number: 1,
                properties: BTreeMap::new(),
                data: vec![],
            },
            Codec::None,
        )
        .unwrap();
    }
    assert!(
        Budget::default()
            .charge(&w.finish(BTreeMap::new(), false).unwrap())
            .is_err()
    );
}
#[test]
fn oversized_compressed_footer_is_refused() {
    let mut w = PuffinWriter::new();
    w.add(
        Blob {
            blob_type: wildcard_puffin::ngram::BLOB_TYPE.into(),
            fields: vec![1],
            snapshot_id: 1,
            sequence_number: 1,
            properties: BTreeMap::from([("padding".into(), "a".repeat(65536))]),
            data: vec![],
        },
        Codec::None,
    )
    .unwrap();
    assert!(
        Budget::default()
            .charge(&w.finish(BTreeMap::new(), true).unwrap())
            .is_err()
    );
}

#[test]
fn current_timestamp_group_blob_is_an_own_segment() {
    let mut w = PuffinWriter::new();
    w.add(
        Blob {
            blob_type: wildcard_puffin::rowgroup::TIMESTAMP_BLOB_TYPE.into(),
            fields: vec![1],
            snapshot_id: 1,
            sequence_number: 1,
            properties: BTreeMap::new(),
            data: vec![],
        },
        Codec::Zstd,
    )
    .unwrap();
    Budget::default()
        .charge(&w.finish(BTreeMap::new(), false).unwrap())
        .unwrap();
}
