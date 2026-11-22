// SPDX-License-Identifier: Apache-2.0
//! Test fixture: extend a real index with empty historical immutable files.
//! This exercises 10,000-entry inventories without changing builder admission.
use libwildcard::puffin::{IndexInfo, ngram::FileEntry, read_first_index, write_index};
use std::{error::Error, fs};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: lake_scale_fixture INPUT.puffin OUTPUT.puffin".into());
    }
    let (meta, mut index) = read_first_index(&fs::read(&args[1])?)?.ok_or("missing index")?;
    for ordinal in index.files.len()..10_000 {
        index.files.push(FileEntry {
            path: format!("s3://historical-empty-test-files/{ordinal:08}.parquet"),
            record_count: 0,
            has_unindexed: false,
        });
    }
    let info = IndexInfo {
        column: meta
            .properties
            .get("column")
            .ok_or("missing column")?
            .clone(),
        field_id: *meta.fields.first().ok_or("missing field")?,
        snapshot_id: meta.snapshot_id,
        sequence_number: meta.sequence_number,
        max_value_bytes: meta
            .properties
            .get("max-value-bytes")
            .ok_or("missing value limit")?
            .parse()?,
    };
    fs::write(&args[2], write_index(&index, &info)?)?;
    Ok(())
}
