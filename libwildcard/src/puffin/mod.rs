// Copyright 2026 The pg_wildcard Authors
// SPDX-License-Identifier: Apache-2.0

//! Iceberg Puffin support for pg_wildcard: a spec-compliant Puffin
//! reader/writer and the `wildcard-ngram-v1` blob (file-level n-gram index).
//!
//! Index files are written **separately** from the table's own Puffin files
//! (deletion vectors, sketches): some readers, including ClickHouse's
//! `PuffinMetadata` format, reject files containing unknown blob types.

#[path = "puffin.rs"]
pub mod codec;
pub mod identity;
pub mod ngram;
/// Compatibility name for the original Puffin format module.
pub use codec as puffin;
pub mod rowgroup;
pub mod segments;

use std::collections::BTreeMap;

use ngram::{BLOB_TYPE, DecodeLimits, NgramError, NgramIndex};
use puffin::{Blob, BlobMetadata, Codec, PuffinError, PuffinWriter};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Puffin(#[from] PuffinError),
    #[error(transparent)]
    Ngram(#[from] NgramError),
}

/// Where an index came from.
#[derive(Clone, Debug)]
pub struct IndexInfo {
    pub column: String,
    /// Iceberg field id of the indexed column.
    pub field_id: i32,
    pub snapshot_id: i64,
    pub sequence_number: i64,
    pub max_value_bytes: usize,
}

/// Serialize an index into a Puffin file.
pub fn write_index(index: &NgramIndex, info: &IndexInfo) -> Result<Vec<u8>, Error> {
    let views: Vec<&str> = index.views.iter().map(|v| v.name()).collect();
    let mut w = PuffinWriter::new();
    w.add(
        Blob {
            blob_type: BLOB_TYPE.into(),
            fields: vec![info.field_id],
            snapshot_id: info.snapshot_id,
            sequence_number: info.sequence_number,
            properties: BTreeMap::from([
                ("column".into(), info.column.clone()),
                ("views".into(), views.join(",")),
                ("files".into(), index.files.len().to_string()),
                ("grams".into(), index.grams.len().to_string()),
                ("max-value-bytes".into(), info.max_value_bytes.to_string()),
            ]),
            data: index.encode(),
        },
        Codec::Zstd,
    )?;
    Ok(w.finish(
        BTreeMap::from([(
            "created-by".into(),
            format!("pg_wildcard {}", env!("CARGO_PKG_VERSION")),
        )]),
        false,
    )?)
}

pub const MAX_INDEX_FILE_BYTES: usize = 16 << 20;
pub const MAX_INDEX_BLOB_BYTES: usize = 32 << 20;
pub const MAX_INDEXES: usize = 16;
pub const MAX_INDEX_HEAP_BYTES: usize = 128 << 20;

fn index_limits(heap_bytes: usize) -> DecodeLimits {
    DecodeLimits {
        max_encoded_bytes: MAX_INDEX_BLOB_BYTES,
        max_files: 128,
        max_grams: 1_000_000,
        max_postings: 1_000_000,
        max_path_bytes: 65536,
        max_heap_bytes: heap_bytes,
    }
}

fn index_footer(file: &[u8]) -> Result<puffin::FileMetadata, Error> {
    if file.len() > MAX_INDEX_FILE_BYTES {
        return Err(PuffinError::Format("index file budget".into()).into());
    }
    Ok(puffin::read_footer(file)?)
}

/// Read the first matching index without decompressing later blobs. The file,
/// payload and decoded collections use the same bounded legacy policy as the
/// builder; a malformed first matching blob is an error, not skipped coverage.
pub fn read_first_index(file: &[u8]) -> Result<Option<(BlobMetadata, NgramIndex)>, Error> {
    let meta = index_footer(file)?;
    let Some(blob) = meta.blobs.into_iter().find(|b| b.blob_type == BLOB_TYPE) else {
        return Ok(None);
    };
    let data = puffin::read_blob_limited(file, &blob, MAX_INDEX_BLOB_BYTES)?;
    let index = NgramIndex::decode_with_policy(&data, index_limits(MAX_INDEX_HEAP_BYTES))?;
    Ok(Some((blob, index)))
}

/// Read every matching blob, bounded to 16 indexes and 128 MiB of aggregate
/// decoded index backing. Input, one decompressed payload, footer metadata and
/// native decoder workspace are separate owners outside that backing budget.
pub fn read_indexes(file: &[u8]) -> Result<Vec<(BlobMetadata, NgramIndex)>, Error> {
    read_indexes_limited(file, MAX_INDEXES, MAX_INDEX_HEAP_BYTES)
}

fn read_indexes_limited(
    file: &[u8],
    max_indexes: usize,
    max_heap_bytes: usize,
) -> Result<Vec<(BlobMetadata, NgramIndex)>, Error> {
    let meta = index_footer(file)?;
    if meta
        .blobs
        .iter()
        .filter(|b| b.blob_type == BLOB_TYPE)
        .count()
        > max_indexes
    {
        return Err(PuffinError::Format("index count budget".into()).into());
    }
    let mut remaining = max_heap_bytes;
    let mut out = Vec::new();
    for b in meta.blobs.into_iter().filter(|b| b.blob_type == BLOB_TYPE) {
        let data = puffin::read_blob_limited(file, &b, MAX_INDEX_BLOB_BYTES)?;
        let index = NgramIndex::decode_with_policy(&data, index_limits(remaining))?;
        remaining = remaining
            .checked_sub(index.retained_heap_bytes())
            .ok_or(NgramError::Corrupt("heap budget"))?;
        out.push((b, index));
    }
    Ok(out)
}

/// Iceberg REST `set-statistics` table update registering a Puffin file in
/// table metadata. Optional: only register when every engine reading the
/// table tolerates unknown blob types (see decision 0007).
pub fn set_statistics_update(
    statistics_path: &str,
    file: &[u8],
) -> Result<serde_json::Value, Error> {
    let meta = puffin::read_footer(file)?;
    let footer_size = {
        let n = file.len();
        i32::from_le_bytes(
            file[n - 12..n - 8]
                .try_into()
                .map_err(|_| PuffinError::Format("invalid footer length".into()))?,
        ) as usize
            + 4
            + 12
    };
    let snapshot_id = meta.blobs.first().map_or(-1, |b| b.snapshot_id);
    let blobs: Vec<_> = meta
        .blobs
        .iter()
        .map(|b| {
            json!({"type": b.blob_type, "snapshot-id": b.snapshot_id, "sequence-number": b.sequence_number,
                   "fields": b.fields, "properties": b.properties})
        })
        .collect();
    Ok(json!({
        "action": "set-statistics",
        "statistics": {
            "snapshot-id": snapshot_id,
            "statistics-path": statistics_path,
            "file-size-in-bytes": file.len(),
            "file-footer-size-in-bytes": footer_size,
            "blob-metadata": blobs
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::View;
    use ngram::NgramIndexBuilder;

    fn multi_index(count: usize, invalid_last: bool) -> Vec<u8> {
        let index = NgramIndexBuilder::new(&[View::Raw], 1024).finish();
        let mut writer = PuffinWriter::new();
        for i in 0..count {
            writer
                .add(
                    Blob {
                        blob_type: BLOB_TYPE.into(),
                        fields: vec![1],
                        snapshot_id: 1,
                        sequence_number: 1,
                        properties: BTreeMap::new(),
                        data: if invalid_last && i + 1 == count {
                            b"bad".to_vec()
                        } else {
                            index.encode()
                        },
                    },
                    Codec::None,
                )
                .unwrap();
        }
        writer.finish(BTreeMap::new(), false).unwrap()
    }

    #[test]
    fn first_index_skips_later_payloads_but_all_indexes_are_atomic() {
        let file = multi_index(2, true);
        assert!(read_first_index(&file).unwrap().is_some());
        assert!(read_indexes(&file).is_err());
        assert!(read_first_index(&multi_index(0, false)).unwrap().is_none());
        assert!(read_first_index(&multi_index(1, true)).is_err());
    }

    #[test]
    fn index_collection_limits_count_and_combined_backing() {
        assert!(read_indexes(&multi_index(MAX_INDEXES, false)).is_ok());
        assert!(
            matches!(read_indexes(&multi_index(MAX_INDEXES + 1, false)), Err(Error::Puffin(PuffinError::Format(message))) if message == "index count budget")
        );
        let file = multi_index(2, false);
        let one = read_first_index(&file)
            .unwrap()
            .unwrap()
            .1
            .retained_heap_bytes();
        assert_eq!(read_indexes_limited(&file, 2, one * 2).unwrap().len(), 2);
        assert!(matches!(
            read_indexes_limited(&file, 2, one * 2 - 1),
            Err(Error::Ngram(NgramError::Corrupt("heap budget")))
        ));
    }

    #[test]
    fn puffin_round_trip_and_statistics() {
        let mut b = NgramIndexBuilder::new(&[View::Raw, View::Lower], 1024);
        let f = b.add_file("s3://lake/t/data/a.parquet");
        b.add_value(f, b"Mozilla/5.0 sqlmap/1.7");
        let idx = b.finish();
        let info = IndexInfo {
            column: "user_agent".into(),
            field_id: 7,
            snapshot_id: 99,
            sequence_number: 3,
            max_value_bytes: 1024,
        };
        let file = write_index(&idx, &info).unwrap();
        let got = read_indexes(&file).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, idx);
        assert_eq!(got[0].0.properties["column"], "user_agent");
        let upd = set_statistics_update("s3://lake/t/metadata/wc-99.puffin", &file).unwrap();
        assert_eq!(upd["statistics"]["snapshot-id"], 99);
        assert_eq!(upd["statistics"]["file-size-in-bytes"], file.len());
    }
}
