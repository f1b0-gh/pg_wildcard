// SPDX-License-Identifier: Apache-2.0
//! Explicit physical layout for selective hot searches; no query semantics.

/// Opt-in MergeTree layout for short security strings and selective searches.
///
/// Adaptive marks let the reader count rows without injecting a physical column
/// into empty PREWHERE steps. Smaller compression blocks limit decompression
/// amplification when selected marks are scattered. Neither setting replaces
/// the exact predicate or assumes that a text index is present.
///
/// This is not a universally optimal layout: benchmark ingestion, storage,
/// broad scans and concurrency on the target data. Existing parts retain their
/// layout. `index_granularity` is read-only after table creation on the qualified
/// engine, so an existing table needs an explicitly managed table migration.
/// This API never executes SQL, rewrites data, disables an index or changes
/// query limits, and accepts no interpolated input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeTreeSearchLayout {
    /// 512 rows/granule, adaptive marks, 16–64 KiB compression blocks.
    Selective,
}

impl MergeTreeSearchLayout {
    /// Trusted SQL settings fragment to append after `SETTINGS` in CREATE TABLE.
    /// No input strings or dynamically selected server settings are accepted.
    pub const fn settings_sql(self) -> &'static str {
        match self {
            Self::Selective => {
                "index_granularity = 512, index_granularity_bytes = 10485760, min_compress_block_size = 16384, max_compress_block_size = 65536"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selective_layout_preserves_adaptive_marks_and_bounded_blocks() {
        let profile = MergeTreeSearchLayout::Selective;
        assert_eq!(
            profile.settings_sql(),
            "index_granularity = 512, index_granularity_bytes = 10485760, min_compress_block_size = 16384, max_compress_block_size = 65536"
        );
        assert!(!profile.settings_sql().contains(';'));
    }
}
