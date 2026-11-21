// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "cold")]

use libwildcard::core::View;
use libwildcard::lake::{DataFile, index_parquet};

// Minimal independently generated Parquet/Thrift fixture, following:
// https://github.com/apache/parquet-format/blob/master/src/main/thrift/parquet.thrift
// One required UTF8 column, one plain dictionary entry, one dictionary-coded row.
#[derive(Default)]
struct Compact {
    bytes: Vec<u8>,
    last: u8,
}
impl Compact {
    fn field(&mut self, id: u8, kind: u8) {
        self.bytes.push(((id - self.last) << 4) | kind);
        self.last = id;
    }
    fn integer(&mut self, id: u8, kind: u8, n: u64) {
        self.field(id, kind);
        varint(n << 1, &mut self.bytes);
    }
    fn string(&mut self, id: u8, value: &str) {
        self.field(id, 8);
        self.bytes.extend(string(value));
    }
    fn structure(&mut self, id: u8, value: Self) {
        self.field(id, 12);
        self.bytes.extend(value.finish());
    }
    fn list(&mut self, id: u8, kind: u8, values: Vec<Vec<u8>>) {
        assert!(values.len() < 15);
        self.field(id, 9);
        self.bytes.push((values.len() as u8) << 4 | kind);
        self.bytes.extend(values.into_iter().flatten());
    }
    fn finish(mut self) -> Vec<u8> {
        self.bytes.push(0);
        self.bytes
    }
}
fn varint(mut n: u64, out: &mut Vec<u8>) {
    while n >= 128 {
        out.push(n as u8 | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
fn string(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    varint(value.len() as u64, &mut out);
    out.extend(value.as_bytes());
    out
}
fn dictionary_file(empty_indices: bool) -> Vec<u8> {
    dictionary_file_with_offset(empty_indices, 4)
}
fn dictionary_file_with_offset(empty_indices: bool, offset: i64) -> Vec<u8> {
    let mut bytes = b"PAR1".to_vec();
    let mut dictionary = Compact::default();
    dictionary.integer(1, 5, 1); // DictionaryPageHeader.num_values
    dictionary.integer(2, 5, 0); // PLAIN
    let mut header = Compact::default();
    header.integer(1, 5, 2); // DICTIONARY_PAGE
    header.integer(2, 5, 5);
    header.integer(3, 5, 5);
    header.structure(7, dictionary);
    bytes.extend(header.finish());
    bytes.extend([1, 0, 0, 0, b'x']);
    let data_offset = bytes.len() as u64;
    let payload: &[u8] = if empty_indices { &[] } else { &[0, 2] }; // width0, RLE1
    let mut data = Compact::default();
    data.integer(1, 5, 1); // one non-null row
    data.integer(2, 5, 8); // RLE_DICTIONARY
    data.integer(3, 5, 3); // RLE definition levels (none for REQUIRED)
    data.integer(4, 5, 3); // RLE repetition levels
    let mut header = Compact::default();
    header.integer(1, 5, 0); // DATA_PAGE
    header.integer(2, 5, payload.len() as u64);
    header.integer(3, 5, payload.len() as u64);
    header.structure(5, data);
    bytes.extend(header.finish());
    bytes.extend(payload);
    let column_bytes = bytes.len() as u64 - 4;

    let mut root = Compact::default();
    root.string(4, "schema");
    root.integer(5, 5, 1);
    let mut leaf = Compact::default();
    leaf.integer(1, 5, 6); // BYTE_ARRAY
    leaf.integer(3, 5, 0); // REQUIRED
    leaf.string(4, "url");
    leaf.integer(6, 5, 0); // UTF8
    let mut column = Compact::default();
    column.integer(1, 5, 6);
    column.list(2, 5, vec![vec![0], vec![16]]); // PLAIN, RLE_DICTIONARY
    column.list(3, 8, vec![string("url")]);
    column.integer(4, 5, 0); // UNCOMPRESSED
    column.integer(5, 6, 1);
    column.integer(6, 6, column_bytes);
    column.integer(7, 6, column_bytes);
    column.integer(9, 6, data_offset);
    column.field(11, 6); // dictionary page offset, including malformed negatives
    varint(((offset << 1) ^ (offset >> 63)) as u64, &mut column.bytes);
    let mut chunk = Compact::default();
    chunk.integer(2, 6, 4);
    chunk.structure(3, column);
    let mut group = Compact::default();
    group.list(1, 12, vec![chunk.finish()]);
    group.integer(2, 6, column_bytes);
    group.integer(3, 6, 1);
    let mut footer = Compact::default();
    footer.integer(1, 5, 1);
    footer.list(2, 12, vec![root.finish(), leaf.finish()]);
    footer.integer(3, 6, 1);
    footer.list(4, 12, vec![group.finish()]);
    let footer = footer.finish();
    bytes.extend(&footer);
    bytes.extend((footer.len() as u32).to_le_bytes());
    bytes.extend(b"PAR1");
    bytes
}

#[test]
fn negative_dictionary_offset_is_an_input_error() {
    let path =
        std::env::temp_dir().join(format!("wc-negative-offset-{}.parquet", std::process::id()));
    std::fs::write(&path, dictionary_file_with_offset(false, -1)).unwrap();
    let outcome = std::panic::catch_unwind(|| {
        index_parquet(
            &[DataFile {
                local: path.to_str().unwrap().into(),
                lake_path: "s3://fixture/negative.parquet".into(),
            }],
            "url",
            &[View::Raw],
            8192,
        )
    });
    std::fs::remove_file(path).unwrap();
    assert!(outcome.expect("negative offsets must not unwind").is_err());
}

fn with_fixture<T>(empty_indices: bool, run: impl FnOnce(DataFile) -> T) -> T {
    let path = std::env::temp_dir().join(format!(
        "wc_dictionary_indices_{}_{}.parquet",
        std::process::id(),
        empty_indices
    ));
    std::fs::write(&path, dictionary_file(empty_indices)).unwrap();
    let result = run(DataFile {
        local: path.to_str().unwrap().to_owned(),
        lake_path: "s3://fixture/one.parquet".into(),
    });
    std::fs::remove_file(path).unwrap();
    result
}

#[test]
fn minimal_dictionary_fixture_indexes_its_one_row() {
    with_fixture(false, |file| {
        let index = index_parquet(&[file], "url", &[View::Raw], 8192).unwrap();
        assert_eq!(index.files[0].record_count, 1);
    });
}

#[test]
fn empty_dictionary_indices_fail_without_unwinding() {
    with_fixture(true, |file| {
        let outcome =
            std::panic::catch_unwind(|| index_parquet(&[file], "url", &[View::Raw], 8192));
        let error = outcome
            .expect("malformed Parquet must return an error, never unwind")
            .unwrap_err();
        assert!(
            matches!(error, libwildcard::lake::LakeError::Invalid(_)),
            "{error}"
        );
    });
}
