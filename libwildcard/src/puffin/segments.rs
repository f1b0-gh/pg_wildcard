// SPDX-License-Identifier: Apache-2.0
//! Aggregate input budgets for segmented sidecars. This preflight does not
//! validate typed indexes, bindings or coverage. Consumers must do those checks
//! before publication/use. Payload bytes are not a bound on decoded heap usage.
use crate::puffin::puffin::{PuffinError, read_blob_limited, read_footer_limited};

pub const MAX_SEGMENTS: usize = 64;
pub const MAX_FILES_PER_SEGMENT: usize = 16;
pub const MAX_FILES: usize = MAX_SEGMENTS * MAX_FILES_PER_SEGMENT;
pub const MAX_PATH_BYTES: usize = 65536;
pub const MAX_ARTIFACTS: usize = MAX_SEGMENTS * 3;
pub const MAX_COMPRESSED_BYTES: usize = 16 << 20;
pub const MAX_PAYLOAD_BYTES: usize = 32 << 20;
pub const MAX_MANIFEST_BYTES: usize = 1 << 20;
pub const MAX_FOOTER_BYTES: usize = 65536;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    compressed_bytes: usize,
    payload_bytes: usize,
    artifacts: usize,
}
impl Budget {
    pub fn compressed_bytes(&self) -> usize {
        self.compressed_bytes
    }
    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }
    pub fn artifacts(&self) -> usize {
        self.artifacts
    }
    pub fn remaining_compressed_bytes(&self) -> usize {
        MAX_COMPRESSED_BYTES - self.compressed_bytes
    }

    /// Charge a single own-blob file. Failed checks never alter counters.
    /// Decode serially and discard the temporary payload before returning.
    pub fn charge(&mut self, file: &[u8]) -> Result<(), PuffinError> {
        let bad = |s: &str| PuffinError::Format(s.into());
        if self.artifacts == MAX_ARTIFACTS || file.len() > self.remaining_compressed_bytes() {
            return Err(bad("segmented compressed/artifact budget exceeded"));
        }
        let footer = read_footer_limited(file, MAX_FOOTER_BYTES)?;
        let [blob] = footer.blobs.as_slice() else {
            return Err(bad("expected one segmented blob"));
        };
        if ![
            crate::puffin::ngram::BLOB_TYPE,
            crate::puffin::rowgroup::BLOB_TYPE,
            crate::puffin::rowgroup::TIMESTAMP_BLOB_TYPE,
            crate::puffin::identity::BLOB_TYPE,
            crate::puffin::identity::LEGACY_BLOB_TYPE,
        ]
        .contains(&blob.blob_type.as_str())
        {
            return Err(bad("unsupported segmented blob"));
        }
        let size = read_blob_limited(file, blob, MAX_PAYLOAD_BYTES - self.payload_bytes)?.len();
        self.compressed_bytes += file.len();
        self.payload_bytes += size;
        self.artifacts += 1;
        Ok(())
    }
}
