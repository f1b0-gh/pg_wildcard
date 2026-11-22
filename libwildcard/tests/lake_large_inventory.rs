#![cfg(feature = "puffin")]
// SPDX-License-Identifier: Apache-2.0
use libwildcard::{
    core::View,
    puffin::{
        IndexInfo,
        ngram::{FileEntry, NgramIndex},
        read_first_index, read_first_index_large_inventory, write_index,
    },
};

#[test]
fn large_inventory_reader_is_opt_in_and_still_bounded() {
    let mut index = NgramIndex {
        views: vec![View::Raw],
        files: (0..129)
            .map(|n| FileEntry {
                path: format!("immutable/{n}"),
                record_count: 0,
                has_unindexed: false,
            })
            .collect(),
        grams: Vec::new(),
    };
    let info = IndexInfo {
        column: "value".into(),
        field_id: 1,
        snapshot_id: 1,
        sequence_number: 1,
        max_value_bytes: 8192,
    };
    let bytes = write_index(&index, &info).unwrap();
    assert!(read_first_index(&bytes).is_err());
    assert_eq!(
        read_first_index_large_inventory(&bytes)
            .unwrap()
            .unwrap()
            .1
            .files
            .len(),
        129
    );
    index.files.resize(65_537, index.files[0].clone());
    let bytes = write_index(&index, &info).unwrap();
    assert!(
        read_first_index_large_inventory(&bytes)
            .unwrap_err()
            .to_string()
            .contains("file budget")
    );
    index.files.truncate(1);
    index.files[0].path = "p".repeat((8 << 20) + 1);
    let bytes = write_index(&index, &info).unwrap();
    assert!(
        read_first_index_large_inventory(&bytes)
            .unwrap_err()
            .to_string()
            .contains("path budget")
    );
}
