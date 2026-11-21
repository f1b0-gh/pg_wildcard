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

use ngram::{BLOB_TYPE, NgramError, NgramIndex};
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

/// Read every `wildcard-ngram-v1` blob of a Puffin file.
pub fn read_indexes(file: &[u8]) -> Result<Vec<(BlobMetadata, NgramIndex)>, Error> {
    let meta = puffin::read_footer(file)?;
    let mut out = Vec::new();
    for b in meta.blobs.into_iter().filter(|b| b.blob_type == BLOB_TYPE) {
        let data = puffin::read_blob(file, &b)?;
        out.push((b, NgramIndex::decode(&data)?));
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
