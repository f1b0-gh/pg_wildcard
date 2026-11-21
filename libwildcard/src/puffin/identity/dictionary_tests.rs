// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::puffin::puffin::read_footer;

fn binding() -> Binding {
    Binding {
        table_uuid: "00000000-0000-4000-8000-000000000001".into(),
        column: "message".into(),
        guard: "event_id".into(),
        column_field_id: 1,
        guard_field_id: 2,
    }
}

#[test]
fn dictionary_wire_does_not_repeat_long_ids_per_gram() {
    let mut builder = Builder::new(binding(), &[View::Raw], 8192).unwrap();
    builder.start_file("file").unwrap();
    let mut index = builder.finish();
    index.files[0].rows = 8;
    for n in 0..1024 {
        let gram = crate::core::grams::key(View::Raw, (n % 256) as u16, (n / 256) as u16, 65);
        index.files[0].postings.insert(
            gram,
            (0..4)
                .map(|i| format!("00000000-0000-4000-8000-{i:012}").into())
                .collect(),
        );
    }
    let payload = index.encode_payload().unwrap();
    assert!(
        payload.len() < 65536,
        "repeated guards must use a dictionary: {} bytes",
        payload.len()
    );
    assert_eq!(&payload[..5], b"WCSI\x02");
    assert_eq!(IdentityIndex::decode_payload(&payload).unwrap(), index);
}

#[test]
fn one_ingested_identity_is_shared_across_grams() {
    let mut b = Builder::new(binding(), &[View::Raw], 8192).unwrap();
    b.start_file("file").unwrap();
    b.add_value(Some(b"powershell -enc command"), Some("one-long-identity"))
        .unwrap();
    let index = b.finish();
    let pointers: Vec<_> = index.files[0]
        .postings
        .values()
        .flat_map(|ids| ids.iter().map(|id| id.as_ptr()))
        .collect();
    assert!(pointers.len() > 10);
    assert!(
        pointers.iter().all(|p| *p == pointers[0]),
        "each posting allocated another identity string"
    );
}

#[test]
fn legacy_fixture_stays_readable_and_new_writer_versions_its_blob() {
    let old = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/identity-v1.puffin"
    ));
    let index = IdentityIndex::read(old).unwrap();
    let plan = index
        .plan(&binding(), Kind::Wildcard, "*powershell*", View::Raw)
        .unwrap();
    assert_eq!(plan.files[0].values.as_ref().unwrap(), &["id\\\0é"]);
    let new = index.write(42, 3).unwrap();
    assert_eq!(
        read_footer(&new).unwrap().blobs[0].blob_type,
        "wildcard-sparse-identity-v2"
    );
    assert_eq!(IdentityIndex::read(&new).unwrap(), index);
}

fn dictionary_payload(dictionary: &[&str], references: &[u32]) -> Vec<u8> {
    fn number(out: &mut Vec<u8>, value: usize) {
        out.extend((value as u32).to_le_bytes());
    }
    fn string(out: &mut Vec<u8>, value: &[u8]) {
        number(out, value.len());
        out.extend(value);
    }
    let mut out = b"WCSI\x02".to_vec();
    string(&mut out, &serde_json::to_vec(&binding()).unwrap());
    number(&mut out, 8192);
    number(&mut out, 1);
    out.push(View::Raw as u8);
    number(&mut out, 1);
    string(&mut out, b"file");
    out.extend(9u64.to_le_bytes());
    out.push(0);
    number(&mut out, dictionary.len());
    for id in dictionary {
        string(&mut out, id.as_bytes());
    }
    number(&mut out, 1);
    out.extend(crate::core::grams::key(View::Raw, 97, 98, 99).to_le_bytes());
    number(&mut out, references.len());
    for ordinal in references {
        out.extend(ordinal.to_le_bytes());
    }
    out
}

#[test]
fn dictionary_references_order_utf8_unused_entries_and_truncation_are_checked() {
    let valid = dictionary_payload(&["a", "z"], &[0, 1]);
    assert!(IdentityIndex::decode_payload(&valid).is_ok());
    for n in 0..valid.len() {
        assert!(
            IdentityIndex::decode_payload(&valid[..n]).is_err(),
            "prefix {n}"
        );
    }
    for malformed in [
        dictionary_payload(&["a", "a"], &[0, 1]),
        dictionary_payload(&["z", "a"], &[0, 1]),
        dictionary_payload(&["a", "b", "c"], &[0, 1]),
        dictionary_payload(&["a", "z"], &[0, 0]),
        dictionary_payload(&["a", "z"], &[1, 0]),
        dictionary_payload(&["a", "z"], &[0, 2]),
        dictionary_payload(&["a", "z"], &[0, u32::MAX]),
        dictionary_payload(&[], &[0]),
    ] {
        assert!(IdentityIndex::decode_payload(&malformed).is_err());
    }
    let header = u32::from_le_bytes(valid[5..9].try_into().unwrap()) as usize;
    let dictionary_count = 9 + header + 4 + 4 + 1 + 4 + 4 + 4 + 8 + 1;
    let mut bad = valid.clone();
    bad[dictionary_count..dictionary_count + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(IdentityIndex::decode_payload(&bad).is_err());
    let mut bad = valid.clone();
    bad[dictionary_count + 8] = 0xff;
    assert!(IdentityIndex::decode_payload(&bad).is_err());
    let mut bad = valid.clone();
    bad[dictionary_count - 1] = 1;
    assert!(IdentityIndex::decode_payload(&bad).is_err());
    let mut bad = valid;
    bad.push(0);
    assert!(IdentityIndex::decode_payload(&bad).is_err());
}

#[test]
fn footer_and_payload_versions_cannot_be_mixed() {
    use crate::puffin::puffin::{Blob, Codec, PuffinWriter, read_blob_limited};
    let old = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/identity-v1.puffin"
    ));
    let old_footer = read_footer(old).unwrap();
    let meta = &old_footer.blobs[0];
    let index = IdentityIndex::read(old).unwrap();
    let old_payload = read_blob_limited(old, meta, MAX_BLOB_BYTES).unwrap();
    assert_eq!(index.encode_payload_version(1).unwrap(), old_payload);
    let new_payload = index.encode_payload().unwrap();
    for (kind, data) in [(BLOB_TYPE, old_payload), (LEGACY_BLOB_TYPE, new_payload)] {
        let mut writer = PuffinWriter::new();
        writer
            .add(
                Blob {
                    blob_type: kind.into(),
                    fields: meta.fields.clone(),
                    snapshot_id: 42,
                    sequence_number: 3,
                    properties: meta.properties.clone(),
                    data,
                },
                Codec::Zstd,
            )
            .unwrap();
        assert!(IdentityIndex::read(&writer.finish(BTreeMap::new(), false).unwrap()).is_err());
    }
}

#[test]
fn both_readers_share_each_decoded_identity_without_expanding_strings() {
    let old = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/identity-v1.puffin"
    ));
    let index = IdentityIndex::read(old).unwrap();
    let new = index.write(42, 3).unwrap();
    for bytes in [old.as_slice(), new.as_slice()] {
        let decoded = IdentityIndex::read(bytes).unwrap();
        let mut pointers = BTreeMap::new();
        for id in decoded.files[0].postings.values().flatten() {
            if let Some(previous) = pointers.insert(id.as_ref(), id.as_ptr()) {
                assert_eq!(previous, id.as_ptr());
            }
        }
        assert_eq!(pointers.len(), 2);
    }
}

#[test]
fn compact_wire_still_charges_every_logical_guard_byte_and_posting() {
    for (grams, ids, width) in [(4097, 32, 256), (32769, 32, 2), (65537, 1, 1)] {
        let mut b = Builder::new(binding(), &[View::Raw], 8192).unwrap();
        b.start_file("file").unwrap();
        let mut index = b.finish();
        let values: BTreeSet<Arc<str>> = (0..ids).map(|n| format!("{n:0width$}").into()).collect();
        index.files[0].postings = (0..grams)
            .map(|n| {
                (
                    crate::core::grams::key(
                        View::Raw,
                        (n % 256) as u16,
                        ((n / 256) % 256) as u16,
                        (n / 65536) as u16,
                    ),
                    values.clone(),
                )
            })
            .collect();
        let compact = index.encode_payload().unwrap();
        assert!(compact.len() < 8 << 20);
        assert!(
            IdentityIndex::decode_payload(&compact).is_err(),
            "logical bound {grams}/{ids}/{width}"
        );
    }
}
