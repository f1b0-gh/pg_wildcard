// SPDX-License-Identifier: Apache-2.0
//! Apache Iceberg Puffin file format (reader and writer).
//!
//! Layout (spec: iceberg.apache.org/puffin-spec):
//! ```text
//! Magic "PFA1" | Blob* | Magic | FooterPayload | PayloadSize (i32 LE) | Flags (4 bytes) | Magic
//! ```
//! The footer payload is UTF-8 JSON (`FileMetadata`), optionally LZ4-compressed
//! as a single frame (flag bit 0 of byte 0). Blob payloads may be compressed
//! with `zstd` or `lz4`; this implementation writes `zstd` or none, and reads
//! `zstd`, `lz4` and none.
//!
//! All reads are bounds-checked and size-limited: Puffin files come from
//! object storage and must be treated as untrusted.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::Read;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAGIC: [u8; 4] = *b"PFA1";
/// Footer struct: payload size (4) + flags (4) + magic (4).
const FOOTER_STRUCT: usize = 12;
/// Upper bounds for untrusted input.
pub const MAX_FOOTER_BYTES: usize = 1 << 20;
pub const MAX_BLOB_BYTES: usize = 64 << 20;
/// Cap the Zstd frame window before decoding. Native context overhead is extra.
pub const MAX_ZSTD_WINDOW_LOG: u32 = 23;

/// Read at most the admitted output, with fallible growth in small chunks.
/// LZ4 frames have a maximum 4 MiB block (8 MiB for the legacy variant accepted
/// by the dependency). Its buffers are bounded by three maximum blocks plus a
/// 64 KiB history window, separately from this output budget.
fn bounded_output(mut reader: impl Read, max_bytes: usize) -> Result<Vec<u8>, PuffinError> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let count = reader.read(&mut chunk[..8192.min(max_bytes - out.len() + 1)])?;
        if count == 0 {
            return Ok(out);
        }
        if count > max_bytes - out.len() {
            return Err(PuffinError::Format("decompressed payload too large".into()));
        }
        if out.len() + count > out.capacity() {
            let target = (out.len() + count)
                .max(out.capacity().saturating_mul(2))
                .min(max_bytes);
            out.try_reserve_exact(target - out.len())
                .map_err(|_| PuffinError::Format("allocation failed".into()))?;
        }
        out.extend_from_slice(&chunk[..count]);
    }
}

#[derive(Debug, Error)]
pub enum PuffinError {
    #[error("not a Puffin file: {0}")]
    Format(String),
    #[error("footer JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported compression codec '{0}'")]
    Codec(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobMetadata {
    #[serde(rename = "type")]
    pub blob_type: String,
    pub fields: Vec<i32>,
    #[serde(rename = "snapshot-id")]
    pub snapshot_id: i64,
    #[serde(rename = "sequence-number")]
    pub sequence_number: i64,
    pub offset: i64,
    pub length: i64,
    #[serde(
        rename = "compression-codec",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub compression_codec: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileMetadata {
    pub blobs: Vec<BlobMetadata>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    None,
    Zstd,
}

/// A blob to write.
pub struct Blob {
    pub blob_type: String,
    pub fields: Vec<i32>,
    pub snapshot_id: i64,
    pub sequence_number: i64,
    pub properties: BTreeMap<String, String>,
    pub data: Vec<u8>,
}

/// In-memory Puffin writer.
pub struct PuffinWriter {
    buf: Vec<u8>,
    meta: FileMetadata,
}

impl Default for PuffinWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl PuffinWriter {
    pub fn new() -> Self {
        Self {
            buf: MAGIC.to_vec(),
            meta: FileMetadata::default(),
        }
    }

    pub fn add(&mut self, blob: Blob, codec: Codec) -> Result<(), PuffinError> {
        let (bytes, name) = match codec {
            Codec::None => (blob.data, None),
            Codec::Zstd => (
                zstd::bulk::compress(&blob.data, 3)?,
                Some("zstd".to_string()),
            ),
        };
        self.meta.blobs.push(BlobMetadata {
            blob_type: blob.blob_type,
            fields: blob.fields,
            snapshot_id: blob.snapshot_id,
            sequence_number: blob.sequence_number,
            offset: self.buf.len() as i64,
            length: bytes.len() as i64,
            compression_codec: name,
            properties: blob.properties,
        });
        self.buf.extend_from_slice(&bytes);
        Ok(())
    }

    /// Finish the file. `compress_footer` writes an LZ4-framed footer (flag bit 0).
    pub fn finish(
        mut self,
        properties: BTreeMap<String, String>,
        compress_footer: bool,
    ) -> Result<Vec<u8>, PuffinError> {
        self.meta.properties = properties;
        let json = serde_json::to_vec(&self.meta)?;
        let payload = if compress_footer {
            let info = lz4_flex::frame::FrameInfo::new().content_size(Some(json.len() as u64));
            let mut enc = lz4_flex::frame::FrameEncoder::with_frame_info(info, Vec::new());
            std::io::Write::write_all(&mut enc, &json)?;
            enc.finish()
                .map_err(|e| PuffinError::Format(e.to_string()))?
        } else {
            json
        };
        self.buf.extend_from_slice(&MAGIC);
        self.buf.extend_from_slice(&payload);
        self.buf
            .extend_from_slice(&(payload.len() as i32).to_le_bytes());
        self.buf
            .extend_from_slice(&[u8::from(compress_footer), 0, 0, 0]);
        self.buf.extend_from_slice(&MAGIC);
        Ok(self.buf)
    }
}

/// Parse the footer of a complete Puffin file.
pub fn read_footer(file: &[u8]) -> Result<FileMetadata, PuffinError> {
    read_footer_limited(file, MAX_FOOTER_BYTES)
}

/// Bound both encoded and decoded footer JSON before parsing metadata.
pub fn read_footer_limited(file: &[u8], max_bytes: usize) -> Result<FileMetadata, PuffinError> {
    let max_bytes = max_bytes.min(MAX_FOOTER_BYTES);
    let bad = |m: &str| PuffinError::Format(m.to_string());
    if file.len() < 4 + 4 + FOOTER_STRUCT || file[..4] != MAGIC || file[file.len() - 4..] != MAGIC {
        return Err(bad("missing magic"));
    }
    let n = file.len();
    let size = i32::from_le_bytes(
        file[n - 12..n - 8]
            .try_into()
            .map_err(|_| bad("invalid footer length"))?,
    );
    if size < 0 || size as usize > max_bytes {
        return Err(bad("invalid footer payload size"));
    }
    let size = size as usize;
    let flags = &file[n - 8..n - 4];
    let start = n
        .checked_sub(FOOTER_STRUCT + size + 4)
        .filter(|&s| s >= 4)
        .ok_or_else(|| bad("footer larger than file"))?;
    if file[start..start + 4] != MAGIC {
        return Err(bad("missing footer magic"));
    }
    let payload = &file[start + 4..start + 4 + size];
    let json = if flags[0] & 1 == 1 {
        Cow::Owned(bounded_output(
            lz4_flex::frame::FrameDecoder::new(payload),
            max_bytes,
        )?)
    } else {
        Cow::Borrowed(payload)
    };
    let meta: FileMetadata = serde_json::from_slice(&json)?;
    for b in &meta.blobs {
        let end = b.offset.checked_add(b.length);
        if b.offset < 4 || b.length < 0 || end.is_none_or(|e| e as usize > start) {
            return Err(bad("blob outside the blob region"));
        }
    }
    Ok(meta)
}

/// Read and decompress one blob's payload.
pub fn read_blob(file: &[u8], b: &BlobMetadata) -> Result<Vec<u8>, PuffinError> {
    read_blob_limited(file, b, MAX_BLOB_BYTES)
}

/// Read a blob with a caller-selected decompression budget, capped by the format
/// budget. Metadata may come from an untrusted caller, not only read_footer.
pub fn read_blob_limited(
    file: &[u8],
    b: &BlobMetadata,
    max_bytes: usize,
) -> Result<Vec<u8>, PuffinError> {
    let bad = || PuffinError::Format("blob outside file".into());
    let start = usize::try_from(b.offset).map_err(|_| bad())?;
    let length = usize::try_from(b.length).map_err(|_| bad())?;
    let end = start.checked_add(length).ok_or_else(bad)?;
    let raw = file.get(start..end).ok_or_else(bad)?;
    let max_bytes = max_bytes.min(MAX_BLOB_BYTES);
    let out = match b.compression_codec.as_deref() {
        None if raw.len() <= max_bytes => raw.to_vec(),
        None => return Err(PuffinError::Format("blob too large".into())),
        Some("zstd") => {
            let mut decoder = zstd::stream::read::Decoder::new(raw)?;
            decoder.window_log_max(MAX_ZSTD_WINDOW_LOG)?;
            bounded_output(decoder, max_bytes)?
        }
        Some("lz4") => bounded_output(lz4_flex::frame::FrameDecoder::new(raw), max_bytes)?,
        Some(other) => return Err(PuffinError::Codec(other.to_string())),
    };
    if out.len() > max_bytes {
        return Err(PuffinError::Format("blob too large".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_rejects_frame_window_even_when_output_is_empty() {
        // Valid empty raw-block frames with unknown content size and explicit
        // 8/16 MiB windows. The unbounded control validates the fixture itself.
        for window_log in [23, 24] {
            let frame = [0x28, 0xb5, 0x2f, 0xfd, 0, (window_log - 10) << 3, 1, 0, 0];
            assert!(zstd::stream::decode_all(&frame[..]).unwrap().is_empty());
            let metadata = BlobMetadata {
                blob_type: "test".into(),
                fields: vec![],
                snapshot_id: 0,
                sequence_number: 0,
                offset: 0,
                length: frame.len() as i64,
                compression_codec: Some("zstd".into()),
                properties: BTreeMap::new(),
            };
            let result = read_blob_limited(&frame, &metadata, 0);
            if window_log == 23 {
                assert_eq!(result.unwrap(), Vec::<u8>::new());
            } else {
                assert!(matches!(result, Err(PuffinError::Io(_))));
            }
        }
    }

    #[test]
    fn decompressed_output_capacity_and_exact_limit_are_bounded() {
        for size in [0, 1, 8191, 8192, 8193, 20_000] {
            let source = vec![b'x'; size];
            let output = bounded_output(source.as_slice(), size).unwrap();
            assert_eq!(output, source);
            assert!(output.capacity() <= size);
            if size > 0 {
                assert!(bounded_output(source.as_slice(), size - 1).is_err());
            }
        }
    }

    #[test]
    fn rejects_large_footer_before_json_allocation() {
        let file = PuffinWriter::new()
            .finish(
                BTreeMap::from([("large".into(), "x".repeat(1 << 20))]),
                false,
            )
            .unwrap();
        assert!(
            matches!(read_footer(&file), Err(PuffinError::Format(message)) if message == "invalid footer payload size")
        );
    }

    #[test]
    fn caller_blob_limit_and_untrusted_offsets() {
        let mut w = PuffinWriter::new();
        w.add(blob(&[7; 1000]), Codec::Zstd).unwrap();
        let file = w.finish(BTreeMap::new(), false).unwrap();
        let mut meta = read_footer(&file).unwrap().blobs.remove(0);
        assert!(read_blob_limited(&file, &meta, 999).is_err());
        assert_eq!(read_blob_limited(&file, &meta, 1000).unwrap().len(), 1000);
        meta.offset = i64::MAX;
        assert!(read_blob_limited(&file, &meta, 1000).is_err());
    }

    fn blob(data: &[u8]) -> Blob {
        Blob {
            blob_type: "test-v1".into(),
            fields: vec![3],
            snapshot_id: 42,
            sequence_number: 7,
            properties: BTreeMap::from([("k".into(), "v".into())]),
            data: data.to_vec(),
        }
    }

    #[test]
    fn round_trip_all_modes() {
        for (codec, compress_footer) in [
            (Codec::None, false),
            (Codec::Zstd, true),
            (Codec::Zstd, false),
            (Codec::None, true),
        ] {
            let mut w = PuffinWriter::new();
            w.add(blob(b"hello"), codec).unwrap();
            w.add(blob(&[7u8; 10_000]), codec).unwrap();
            let f = w
                .finish(
                    BTreeMap::from([("created-by".into(), "pg_wildcard".into())]),
                    compress_footer,
                )
                .unwrap();
            let m = read_footer(&f).unwrap();
            assert_eq!(m.blobs.len(), 2);
            assert_eq!(m.properties["created-by"], "pg_wildcard");
            assert_eq!(read_blob(&f, &m.blobs[0]).unwrap(), b"hello");
            assert_eq!(read_blob(&f, &m.blobs[1]).unwrap(), vec![7u8; 10_000]);
        }
    }

    #[test]
    fn empty_file_is_valid() {
        let f = PuffinWriter::new().finish(BTreeMap::new(), false).unwrap();
        assert_eq!(&f[..4], b"PFA1");
        assert!(read_footer(&f).unwrap().blobs.is_empty());
    }

    #[test]
    fn footer_json_uses_spec_field_names() {
        let mut w = PuffinWriter::new();
        w.add(blob(b"x"), Codec::Zstd).unwrap();
        let f = w.finish(BTreeMap::new(), false).unwrap();
        let s = String::from_utf8_lossy(&f);
        for key in [
            "\"snapshot-id\"",
            "\"sequence-number\"",
            "\"compression-codec\":\"zstd\"",
            "\"type\":\"test-v1\"",
            "\"fields\":[3]",
        ] {
            assert!(s.contains(key), "{key}");
        }
    }

    #[test]
    fn rejects_corruption() {
        let mut w = PuffinWriter::new();
        w.add(blob(b"hello"), Codec::None).unwrap();
        let f = w.finish(BTreeMap::new(), false).unwrap();
        for cut in [0, 5, f.len() - 1] {
            assert!(read_footer(&f[..cut]).is_err());
        }
        let mut bad = f.clone();
        let n = bad.len();
        bad[n - 12..n - 8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(read_footer(&bad).is_err());
        let mut bad = f.clone();
        bad[0] = b'X';
        assert!(read_footer(&bad).is_err());
    }
}
