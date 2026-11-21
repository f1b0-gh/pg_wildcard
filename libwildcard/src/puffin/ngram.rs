// SPDX-License-Identifier: Apache-2.0
//! `wildcard-ngram-v1`: a file-level n-gram index stored as a Puffin blob.
//!
//! For one column of an Iceberg table snapshot, it maps every gram (view +
//! byte trigram, as in `wildcard-core`) to the data files whose values contain
//! it. Pruning evaluates a pattern's gram plan over these sets and returns the
//! candidate files; files are never wrongly excluded (soundness), so the
//! query engine reading only the candidates returns exactly the full answer.
//!
//! Payload (little endian, varints are unsigned LEB128):
//! ```text
//! "WCNG" | version u8 = 1 | views bitmask u8 | reserved u16
//! n_files | (path_len, path, record_count, flags u8)*         flags bit 0: has unindexed (overlong) values
//! n_grams | (gram_delta, n_postings, (file_ord_delta)*)*       grams ascending, file ordinals ascending
//! ```

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

use std::collections::BTreeMap;

use crate::core::View;
use crate::core::grams::{self, GramKey};
use crate::core::query::GramQuery;
use crate::core::verify::{Kind, MatchError, Matcher};
use thiserror::Error;

pub const BLOB_TYPE: &str = "wildcard-ngram-v1";
const MAGIC: &[u8; 4] = b"WCNG";

/// Decoder admission policy. Heap bytes cover requested vector/string backing,
/// including spare capacity, not allocator metadata, input or decompressor memory.
#[derive(Clone, Copy, Debug)]
pub struct DecodeLimits {
    pub max_encoded_bytes: usize,
    pub max_files: usize,
    pub max_grams: usize,
    pub max_postings: usize,
    pub max_path_bytes: usize,
    pub max_heap_bytes: usize,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_encoded_bytes: 64 << 20,
            max_files: 65_536,
            max_grams: 1_000_000,
            max_postings: 8_000_000,
            max_path_bytes: 8 << 20,
            max_heap_bytes: 128 << 20,
        }
    }
}

fn charge(remaining: &mut usize, bytes: usize) -> Result<(), NgramError> {
    *remaining = remaining
        .checked_sub(bytes)
        .ok_or(NgramError::Corrupt("heap budget"))?;
    Ok(())
}

/// Grow only after validating a record, never directly from an advertised count.
fn grow<T>(values: &mut Vec<T>, remaining: &mut usize) -> Result<(), NgramError> {
    if values.len() == values.capacity() {
        let additional = values
            .capacity()
            .max(1)
            .min(*remaining / std::mem::size_of::<T>())
            .max(1);
        let bytes = additional
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(NgramError::Corrupt("heap budget"))?;
        charge(remaining, bytes)?;
        values
            .try_reserve_exact(additional)
            .map_err(|_| NgramError::Corrupt("allocation failed"))?;
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum NgramError {
    #[error("corrupt wildcard-ngram-v1 blob: {0}")]
    Corrupt(&'static str),
    #[error(transparent)]
    Pattern(#[from] MatchError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub record_count: u64,
    /// Some values exceeded the indexing limit: the file is always a candidate.
    pub has_unindexed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NgramIndex {
    pub views: Vec<View>,
    pub files: Vec<FileEntry>,
    /// Sorted by gram; postings sorted file ordinals.
    pub grams: Vec<(GramKey, Vec<u32>)>,
}

/// Pruning outcome.
#[derive(Debug)]
pub struct Pruned<'a> {
    pub files: Vec<&'a FileEntry>,
    /// `false` when the pattern yields no usable grams (every file is a candidate).
    pub used_index: bool,
}

impl NgramIndex {
    pub fn candidates(
        &self,
        kind: Kind,
        pattern: &str,
        view: View,
    ) -> Result<Pruned<'_>, NgramError> {
        let m = Matcher::new(kind, pattern, view)?;
        let n = self.files.len() as u32;
        if !self.views.contains(&view) || m.plan.is_all() {
            return Ok(Pruned {
                files: self.files.iter().collect(),
                used_index: false,
            });
        }
        let lookup = |g: GramKey| match self.grams.binary_search_by_key(&g, |e| e.0) {
            Ok(i) => self.grams[i].1.as_slice(),
            Err(_) => &[],
        };
        let fast = match &m.plan {
            GramQuery::And(parts) => intersect_flat(parts, &lookup),
            _ => None,
        };
        let mut ids = fast.unwrap_or_else(|| m.plan.candidates(n, &mut |g| lookup(g).to_vec()));
        ids.extend(
            self.files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.has_unindexed)
                .map(|(i, _)| i as u32),
        );
        ids.sort_unstable();
        ids.dedup();
        Ok(Pruned {
            files: ids.into_iter().map(|i| &self.files[i as usize]).collect(),
            used_index: true,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(1);
        out.push(self.views.iter().fold(0u8, |m, v| m | 1 << (*v as u8)));
        out.extend_from_slice(&[0, 0]);
        put(&mut out, self.files.len() as u64);
        for f in &self.files {
            put(&mut out, f.path.len() as u64);
            out.extend_from_slice(f.path.as_bytes());
            put(&mut out, f.record_count);
            out.push(u8::from(f.has_unindexed));
        }
        put(&mut out, self.grams.len() as u64);
        let mut prev = 0u64;
        for (g, post) in &self.grams {
            put(&mut out, g - prev);
            prev = *g;
            put(&mut out, post.len() as u64);
            let mut p = 0u32;
            for (i, &f) in post.iter().enumerate() {
                put(&mut out, u64::from(if i == 0 { f } else { f - p }));
                p = f;
            }
        }
        out
    }

    pub fn decode(b: &[u8]) -> Result<Self, NgramError> {
        Self::decode_with_policy(b, DecodeLimits::default())
    }

    /// Reject cardinalities before allocating postings or entries.
    pub fn decode_with_limits(
        b: &[u8],
        max_files: usize,
        max_grams: usize,
        max_postings: usize,
    ) -> Result<Self, NgramError> {
        Self::decode_with_policy(
            b,
            DecodeLimits {
                max_files,
                max_grams,
                max_postings,
                ..DecodeLimits::default()
            },
        )
    }

    pub fn decode_with_policy(b: &[u8], limits: DecodeLimits) -> Result<Self, NgramError> {
        Self::decode_inner(b, limits, false)
    }

    pub(crate) fn decode_canonical_with_limits(
        b: &[u8],
        max_files: usize,
        max_grams: usize,
        max_postings: usize,
    ) -> Result<Self, NgramError> {
        Self::decode_inner(
            b,
            DecodeLimits {
                max_files,
                max_grams,
                max_postings,
                ..DecodeLimits::default()
            },
            true,
        )
    }

    fn decode_inner(b: &[u8], limits: DecodeLimits, canonical: bool) -> Result<Self, NgramError> {
        if b.len() > limits.max_encoded_bytes {
            return Err(NgramError::Corrupt("encoded byte budget"));
        }
        let mut remaining = limits.max_heap_bytes;
        let mut r = Reader {
            b,
            pos: 0,
            canonical,
        };
        if r.take(4)? != MAGIC {
            return Err(NgramError::Corrupt("magic"));
        }
        if r.byte()? != 1 {
            return Err(NgramError::Corrupt("unsupported version"));
        }
        let mask = r.byte()?;
        let reserved = r.take(2)?;
        if canonical
            && (reserved != [0, 0]
                || mask & !View::ALL.into_iter().fold(0u8, |m, v| m | 1 << v as u8) != 0)
        {
            return Err(NgramError::Corrupt("noncanonical header"));
        }
        let views: Vec<View> = View::ALL
            .into_iter()
            .filter(|v| mask & (1 << (*v as u8)) != 0)
            .collect();
        charge(
            &mut remaining,
            views.capacity() * std::mem::size_of::<View>(),
        )?;
        let n_files = r.records(3)?;
        if n_files > limits.max_files || n_files > u32::MAX as usize {
            return Err(NgramError::Corrupt("file budget"));
        }
        let mut files = Vec::new();
        let mut path_bytes = 0usize;
        for _ in 0..n_files {
            let len = r.count()?;
            path_bytes = path_bytes
                .checked_add(len)
                .filter(|n| *n <= limits.max_path_bytes)
                .ok_or(NgramError::Corrupt("path budget"))?;
            let raw_path =
                std::str::from_utf8(r.take(len)?).map_err(|_| NgramError::Corrupt("path utf-8"))?;
            let record_count = r.varint()?;
            let flags = r.byte()?;
            if canonical && flags > 1 {
                return Err(NgramError::Corrupt("noncanonical flags"));
            }
            let has_unindexed = flags & 1 == 1;
            grow(&mut files, &mut remaining)?;
            charge(&mut remaining, len)?;
            let mut path = String::new();
            path.try_reserve_exact(len)
                .map_err(|_| NgramError::Corrupt("allocation failed"))?;
            path.push_str(raw_path);
            files.push(FileEntry {
                path,
                record_count,
                has_unindexed,
            });
        }
        let n_grams = r.records(2)?;
        if n_grams > limits.max_grams {
            return Err(NgramError::Corrupt("gram budget"));
        }
        let mut total_postings = 0usize;
        let mut grams = Vec::new();
        let mut prev = 0u64;
        for i in 0..n_grams {
            let d = r.varint()?;
            if i > 0 && d == 0 {
                return Err(NgramError::Corrupt("grams not strictly ascending"));
            }
            prev = prev
                .checked_add(d)
                .ok_or(NgramError::Corrupt("gram overflow"))?;
            let np = r.count()?;
            if np > files.len() {
                return Err(NgramError::Corrupt("posting count exceeds files"));
            }
            total_postings = total_postings
                .checked_add(np)
                .filter(|n| *n <= limits.max_postings)
                .ok_or(NgramError::Corrupt("posting budget"))?;
            let mut post = Vec::new();
            let mut f = 0u64;
            for j in 0..np {
                let d = r.varint()?;
                if j > 0 && d == 0 {
                    return Err(NgramError::Corrupt("postings not strictly ascending"));
                }
                f = if j == 0 {
                    d
                } else {
                    f.checked_add(d)
                        .ok_or(NgramError::Corrupt("posting overflow"))?
                };
                if f >= files.len() as u64 {
                    return Err(NgramError::Corrupt("file ordinal out of range"));
                }
                grow(&mut post, &mut remaining)?;
                post.push(f as u32);
            }
            grow(&mut grams, &mut remaining)?;
            grams.push((prev, post));
        }
        if r.pos != b.len() {
            return Err(NgramError::Corrupt("trailing bytes"));
        }
        Ok(Self {
            views,
            files,
            grams,
        })
    }

    /// Requested backing retained by this index; excludes allocator metadata.
    pub fn retained_heap_bytes(&self) -> usize {
        self.views.capacity() * std::mem::size_of::<View>()
            + self.files.capacity() * std::mem::size_of::<FileEntry>()
            + self.files.iter().map(|f| f.path.capacity()).sum::<usize>()
            + self.grams.capacity() * std::mem::size_of::<(GramKey, Vec<u32>)>()
            + self
                .grams
                .iter()
                .map(|(_, p)| p.capacity() * std::mem::size_of::<u32>())
                .sum::<usize>()
    }
}

/// Flat conjunctions can use exact decoded posting lengths without changing
/// the serialized index or Boolean plan. Nested plans retain the general path.
fn intersect_flat<'a>(
    parts: &[GramQuery],
    lookup: &impl Fn(GramKey) -> &'a [u32],
) -> Option<Vec<u32>> {
    if parts.is_empty() || parts.iter().any(|p| !matches!(p, GramQuery::Gram(_))) {
        return None;
    }
    let mut postings = Vec::with_capacity(parts.len());
    for part in parts {
        if let GramQuery::Gram(g) = part {
            let ids = lookup(*g);
            if ids.is_empty() {
                return Some(Vec::new());
            }
            postings.push(ids);
        }
    }
    postings.sort_unstable_by_key(|ids| ids.len());
    let (first, rest) = postings.split_first()?;
    let mut result = first.to_vec();
    for ids in rest {
        if result.is_empty() {
            break;
        }
        // Broad patterns often repeat the same full-coverage list. An exact
        // slice comparison avoids rebuilding an already identical selection.
        if *ids == result.as_slice() {
            continue;
        }
        // For skewed lists, probes avoid scanning a large prefix. For similar
        // sizes, a monotone cursor keeps the intersection linear. Reuse one
        // output allocation and never clone the other posting lists.
        if ids.len() / result.len() >= 8 {
            result.retain(|id| ids.binary_search(id).is_ok());
        } else {
            let (mut left, mut right, mut kept) = (0, 0, 0);
            while left < result.len() && right < ids.len() {
                match result[left].cmp(&ids[right]) {
                    std::cmp::Ordering::Less => left += 1,
                    std::cmp::Ordering::Greater => right += 1,
                    std::cmp::Ordering::Equal => {
                        // kept <= left: writes cannot overwrite unread values.
                        result[kept] = result[left];
                        kept += 1;
                        left += 1;
                        right += 1;
                    }
                }
            }
            result.truncate(kept);
        }
    }
    Some(result)
}

/// Compatibility builder for trusted callers that provide their own aggregate
/// admission. Use `BoundedNgramIndexBuilder` for untrusted or large data sets.
pub struct NgramIndexBuilder {
    views: Vec<View>,
    max_value_bytes: usize,
    files: Vec<FileEntry>,
    grams: BTreeMap<GramKey, Vec<u32>>,
    scratch: Vec<GramKey>,
}

/// Bounded legacy construction. At most 128 files, 64 KiB of paths, one million
/// grams/postings and 8192 bytes per value. Counts bound BTree nodes and posting
/// capacities; they are not a process RSS bound. Budget exhaustion in grams
/// makes the current file always-recheck, preserving every possible match.
pub struct BoundedNgramIndexBuilder {
    inner: NgramIndexBuilder,
    // Coverage flags also describe isolated overlong values. Only exhausted
    // indexing budgets suppress later short values in the same file.
    exhausted_files: [bool; 128],
    path_bytes: usize,
    postings: usize,
    max_grams: usize,
    max_postings: usize,
}

impl BoundedNgramIndexBuilder {
    pub fn new(views: &[View], max_value_bytes: usize) -> Result<Self, NgramError> {
        if views.is_empty()
            || views.len() > View::ALL.len()
            || max_value_bytes == 0
            || max_value_bytes > 8192
        {
            return Err(NgramError::Corrupt("builder configuration"));
        }
        let mut inner = NgramIndexBuilder::new(views, max_value_bytes);
        inner.views.sort_unstable_by_key(|v| *v as u8);
        inner.views.dedup();
        Ok(Self {
            inner,
            exhausted_files: [false; 128],
            path_bytes: 0,
            postings: 0,
            max_grams: 1_000_000,
            max_postings: 1_000_000,
        })
    }

    pub fn try_add_file(&mut self, path: &str) -> Result<u32, NgramError> {
        if self.inner.files.len() >= 128 {
            return Err(NgramError::Corrupt("file budget"));
        }
        let path_bytes = self
            .path_bytes
            .checked_add(path.len())
            .filter(|n| *n <= 65536)
            .ok_or(NgramError::Corrupt("path budget"))?;
        let mut owned = String::new();
        owned
            .try_reserve_exact(path.len())
            .map_err(|_| NgramError::Corrupt("allocation failed"))?;
        owned.push_str(path);
        self.inner
            .files
            .try_reserve(1)
            .map_err(|_| NgramError::Corrupt("allocation failed"))?;
        let file = self.inner.files.len() as u32;
        self.inner.files.push(FileEntry {
            path: owned,
            record_count: 0,
            has_unindexed: false,
        });
        self.path_bytes = path_bytes;
        Ok(file)
    }

    pub fn try_add_null(&mut self, file: u32) -> Result<(), NgramError> {
        let f = self
            .inner
            .files
            .get_mut(file as usize)
            .ok_or(NgramError::Corrupt("file ordinal out of range"))?;
        f.record_count = f
            .record_count
            .checked_add(1)
            .ok_or(NgramError::Corrupt("record count overflow"))?;
        Ok(())
    }

    pub fn try_add_value(&mut self, file: u32, value: &[u8]) -> Result<(), NgramError> {
        self.try_add_null(file)?;
        let f = &mut self.inner.files[file as usize];
        if value.len() > self.inner.max_value_bytes {
            f.has_unindexed = true;
            return Ok(());
        }
        if self.exhausted_files[file as usize] {
            return Ok(());
        }
        self.inner.scratch.clear();
        for &view in &self.inner.views {
            self.inner
                .scratch
                .extend(grams::value_grams(view, &view.apply(value)));
        }
        for &gram in &self.inner.scratch {
            let found = self
                .inner
                .grams
                .get(&gram)
                .map(|post| post.binary_search(&file));
            if matches!(found, Some(Ok(_))) {
                continue;
            }
            if self.postings == self.max_postings
                || (found.is_none() && self.inner.grams.len() == self.max_grams)
            {
                self.inner.files[file as usize].has_unindexed = true;
                self.exhausted_files[file as usize] = true;
                break;
            }
            let post = self.inner.grams.entry(gram).or_default();
            if post.try_reserve(1).is_err() {
                self.inner.files[file as usize].has_unindexed = true;
                self.exhausted_files[file as usize] = true;
                return Err(NgramError::Corrupt("allocation failed"));
            }
            let position = match found {
                Some(Err(i)) => i,
                _ => 0,
            };
            post.insert(position, file);
            self.postings += 1;
        }
        Ok(())
    }

    pub fn finish(self) -> NgramIndex {
        self.inner.finish()
    }
}

impl NgramIndexBuilder {
    pub fn new(views: &[View], max_value_bytes: usize) -> Self {
        Self {
            views: views.to_vec(),
            max_value_bytes,
            files: Vec::new(),
            grams: BTreeMap::new(),
            scratch: Vec::new(),
        }
    }

    /// Start a new data file; returns its ordinal.
    pub fn add_file(&mut self, path: &str) -> u32 {
        self.files.push(FileEntry {
            path: path.to_string(),
            record_count: 0,
            has_unindexed: false,
        });
        (self.files.len() - 1) as u32
    }

    pub fn add_value(&mut self, file: u32, value: &[u8]) {
        let f = &mut self.files[file as usize];
        f.record_count += 1;
        if value.len() > self.max_value_bytes {
            f.has_unindexed = true;
            return;
        }
        self.scratch.clear();
        for &v in &self.views {
            self.scratch.extend(grams::value_grams(v, &v.apply(value)));
        }
        for &g in &self.scratch {
            let post = self.grams.entry(g).or_default();
            if post.last() != Some(&file) {
                post.push(file);
            }
        }
    }

    /// Count a null / missing value (no grams).
    pub fn add_null(&mut self, file: u32) {
        self.files[file as usize].record_count += 1;
    }

    pub fn finish(self) -> NgramIndex {
        NgramIndex {
            views: self.views,
            files: self.files,
            grams: self.grams.into_iter().collect(),
        }
    }
}

fn put(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
    canonical: bool,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], NgramError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or(NgramError::Corrupt("truncated"))?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn byte(&mut self) -> Result<u8, NgramError> {
        Ok(self.take(1)?[0])
    }
    fn varint(&mut self) -> Result<u64, NgramError> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            if shift == 63 && b > 1 {
                return Err(NgramError::Corrupt("varint overflow"));
            }
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                if self.canonical && shift > 0 && b == 0 {
                    return Err(NgramError::Corrupt("noncanonical varint"));
                }
                return Ok(v);
            }
        }
        Err(NgramError::Corrupt("varint overflow"))
    }
    /// A count that must fit in the remaining bytes (guards allocations).
    fn count(&mut self) -> Result<usize, NgramError> {
        let v = self.varint()?;
        if v > (self.b.len() - self.pos) as u64 {
            return Err(NgramError::Corrupt("count larger than remaining data"));
        }
        Ok(v as usize)
    }

    fn records(&mut self, min_bytes: usize) -> Result<usize, NgramError> {
        let count = self.count()?;
        if count > (self.b.len() - self.pos) / min_bytes {
            return Err(NgramError::Corrupt("record count exceeds remaining data"));
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_posting_delta_overflow() {
        let idx = NgramIndex {
            views: vec![View::Raw],
            files: sample().files[..2].to_vec(),
            grams: vec![(1, vec![1])],
        };
        let mut bytes = idx.encode();
        bytes.truncate(bytes.len() - 2); // replace posting count and ordinal
        put(&mut bytes, 2);
        put(&mut bytes, 1);
        put(&mut bytes, u64::MAX);
        assert!(NgramIndex::decode(&bytes).is_err());
    }

    #[test]
    fn rejects_varint_tenth_byte_overflow() {
        let mut bytes = vec![0x80; 9];
        bytes.push(2);
        assert!(
            Reader {
                b: &bytes,
                pos: 0,
                canonical: false
            }
            .varint()
            .is_err()
        );
        bytes.clear();
        put(&mut bytes, u64::MAX);
        assert_eq!(
            Reader {
                b: &bytes,
                pos: 0,
                canonical: false
            }
            .varint()
            .unwrap(),
            u64::MAX
        );
    }

    fn sample() -> NgramIndex {
        let mut b = NgramIndexBuilder::new(&[View::Raw, View::Decoded, View::RevDomain], 64);
        let f0 = b.add_file("s3://lake/http/f0.parquet");
        for v in ["https://good.example/a", "https://good.example/b"] {
            b.add_value(f0, v.as_bytes());
        }
        let f1 = b.add_file("s3://lake/http/f1.parquet");
        b.add_value(f1, b"https://login.evil.com/%77p-admin/x.php");
        b.add_null(f1);
        let f2 = b.add_file("s3://lake/http/f2.parquet");
        b.add_value(f2, "x".repeat(100).as_bytes()); // overlong
        b.finish()
    }

    #[test]
    fn prunes_soundly() {
        let idx = sample();
        let p = idx
            .candidates(Kind::Wildcard, "*/wp-admin/*", View::Decoded)
            .unwrap();
        let paths: Vec<_> = p.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["s3://lake/http/f1.parquet", "s3://lake/http/f2.parquet"]
        );
        assert!(p.used_index);
        let p = idx
            .candidates(Kind::Like, "com.evil.%", View::RevDomain)
            .unwrap();
        assert_eq!(p.files.len(), 2);
        // view not indexed -> every file
        let p = idx.candidates(Kind::Like, "%evil%", View::Lower).unwrap();
        assert_eq!((p.files.len(), p.used_index), (3, false));
        assert_eq!(idx.files[1].record_count, 2);
    }

    #[test]
    fn encode_decode_round_trip() {
        let idx = sample();
        let bytes = idx.encode();
        assert_eq!(NgramIndex::decode(&bytes).unwrap(), idx);
        for cut in [0, 3, 8, bytes.len() / 2, bytes.len() - 1] {
            assert!(NgramIndex::decode(&bytes[..cut]).is_err(), "cut {cut}");
        }
        let mut huge = bytes.clone();
        huge[8] = 0xff; // corrupt n_files varint
        assert!(NgramIndex::decode(&huge).is_err());
    }
}

#[cfg(test)]
mod bounded_decode_tests {
    use super::*;

    #[test]
    fn decoded_backing_and_paths_obey_explicit_budgets() {
        let index = NgramIndex {
            views: vec![View::Raw],
            files: vec![FileEntry {
                path: "path".into(),
                record_count: 1,
                has_unindexed: false,
            }],
            grams: vec![],
        };
        let data = index.encode();
        let bytes = NgramIndex::decode(&data).unwrap().retained_heap_bytes();
        let limits = DecodeLimits {
            max_heap_bytes: bytes,
            max_path_bytes: 4,
            ..DecodeLimits::default()
        };
        assert_eq!(
            NgramIndex::decode_with_policy(&data, limits).unwrap(),
            index
        );
        assert!(matches!(
            NgramIndex::decode_with_policy(
                &data,
                DecodeLimits {
                    max_heap_bytes: bytes - 1,
                    ..limits
                }
            ),
            Err(NgramError::Corrupt("heap budget"))
        ));
        assert!(matches!(
            NgramIndex::decode_with_policy(
                &data,
                DecodeLimits {
                    max_path_bytes: 3,
                    ..limits
                }
            ),
            Err(NgramError::Corrupt("path budget"))
        ));
        assert!(matches!(
            NgramIndex::decode_with_policy(
                &data,
                DecodeLimits {
                    max_encoded_bytes: data.len() - 1,
                    ..limits
                }
            ),
            Err(NgramError::Corrupt("encoded byte budget"))
        ));
    }

    #[test]
    fn canonical_validation_rejects_alternate_encodings_without_reencoding() {
        let index = NgramIndex {
            views: vec![View::Raw],
            files: vec![FileEntry {
                path: "a".into(),
                record_count: 0,
                has_unindexed: false,
            }],
            grams: vec![],
        };
        let data = index.encode();
        assert_eq!(
            NgramIndex::decode_canonical_with_limits(&data, 1, 1, 1).unwrap(),
            index
        );
        let mut reserved = data.clone();
        reserved[6] = 1;
        let mut flags = data.clone();
        flags[12] = 2;
        let mut nonminimal = data.clone();
        nonminimal.splice(8..9, [0x81, 0]);
        for modified in [reserved, flags, nonminimal] {
            assert!(NgramIndex::decode(&modified).is_ok());
            assert!(NgramIndex::decode_canonical_with_limits(&modified, 1, 1, 1).is_err());
        }
    }

    #[test]
    fn bounded_builder_matches_legacy_bytes_with_short_values_after_overlong_values() {
        for long_position in 0..3 {
            let mut bounded = BoundedNgramIndexBuilder::new(&View::ALL, 8).unwrap();
            let mut legacy = NgramIndexBuilder::new(&View::ALL, 8);
            let file = bounded.try_add_file("a").unwrap();
            assert_eq!(legacy.add_file("a"), file);
            for (position, short) in [b"ABC".as_slice(), b"%44%45", b"xyz\\a"]
                .into_iter()
                .enumerate()
            {
                if position == long_position {
                    bounded.try_add_value(file, b"overlong value").unwrap();
                    legacy.add_value(file, b"overlong value");
                }
                bounded.try_add_value(file, short).unwrap();
                legacy.add_value(file, short);
            }
            bounded.try_add_null(file).unwrap();
            legacy.add_null(file);
            let actual = bounded.finish();
            let expected = legacy.finish();
            assert!(actual.files[0].has_unindexed);
            assert_eq!(actual.files[0].record_count, 5);
            assert_eq!(
                actual.encode(),
                expected.encode(),
                "long position {long_position}"
            );
        }
    }

    #[test]
    fn bounded_builder_keeps_budget_exhausted_and_overlong_files_as_candidates() {
        for gram_limit in [true, false] {
            let mut builder = BoundedNgramIndexBuilder::new(&[View::Raw], 8).unwrap();
            if gram_limit {
                builder.max_grams = 1;
            } else {
                builder.max_postings = 1;
            }
            let file = builder.try_add_file("a").unwrap();
            builder.try_add_value(file, b"abcdef").unwrap();
            builder.try_add_value(file, b"other").unwrap();
            let overlong = builder.try_add_file("b").unwrap();
            builder.try_add_value(overlong, b"overlong value").unwrap();
            builder.try_add_null(overlong).unwrap();
            let index = builder.finish();
            assert_eq!(index.grams.len(), 1);
            assert!(
                index
                    .files
                    .iter()
                    .all(|f| f.has_unindexed && f.record_count == 2)
            );
            assert_eq!(
                index
                    .candidates(Kind::Wildcard, "*missing*", View::Raw)
                    .unwrap()
                    .files
                    .len(),
                2
            );
        }
    }

    #[test]
    fn bounded_builder_checks_metadata_and_revisiting_files_keeps_sorted_postings() {
        assert!(BoundedNgramIndexBuilder::new(&[], 8).is_err());
        assert!(BoundedNgramIndexBuilder::new(&[View::Raw], 8193).is_err());
        let mut builder = BoundedNgramIndexBuilder::new(&[View::Raw], 8).unwrap();
        let first = builder.try_add_file("a").unwrap();
        let second = builder.try_add_file("b").unwrap();
        for file in [second, first, second] {
            builder.try_add_value(file, b"abc").unwrap();
        }
        assert!(builder.try_add_value(7, b"abc").is_err());
        assert!(builder.try_add_file(&"x".repeat(65535)).is_err());
        let index = builder.finish();
        assert!(index.grams.iter().all(|(_, p)| p == &[first, second]));
        let mut builder = BoundedNgramIndexBuilder::new(&[View::Raw], 8).unwrap();
        for _ in 0..128 {
            builder.try_add_file("").unwrap();
        }
        assert!(builder.try_add_file("").is_err());
    }
    #[test]
    fn rejects_padded_impossible_record_counts_before_parsing_records() {
        for files in [false, true] {
            let mut data = b"WCNG\x01\x01\x00\x00".to_vec();
            if !files {
                put(&mut data, 0);
            }
            put(&mut data, 8);
            data.extend_from_slice(&[0; 8]);
            assert!(matches!(
                NgramIndex::decode(&data),
                Err(NgramError::Corrupt("record count exceeds remaining data"))
            ));
        }
    }
    #[test]
    fn rejects_cardinality_before_decoding_untrusted_index() {
        let mut b = NgramIndexBuilder::new(&[View::Raw], 1024);
        let f = b.add_file("file");
        b.add_value(f, b"powershell.exe");
        let idx = b.finish();
        let data = idx.encode();
        assert!(NgramIndex::decode_with_limits(&data, 0, 100, 100).is_err());
        assert!(NgramIndex::decode_with_limits(&data, 1, 0, 100).is_err());
        assert!(NgramIndex::decode_with_limits(&data, 1, 100, 0).is_err());
        assert_eq!(
            NgramIndex::decode_with_limits(&data, 1, 100, 100).unwrap(),
            idx
        );
    }
}

#[cfg(test)]
mod fast_skip_tests {
    use super::*;
    use crate::core::query::GramQuery;

    #[test]
    fn flat_intersections_equal_per_id_boolean_oracle() {
        let mut seed = 0x2026_1115_u64;
        for case in 0..512 {
            let mut postings = Vec::new();
            for gram in 0..8 {
                let mut ids = Vec::new();
                for id in 0..128 {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    if seed.is_multiple_of(gram + 2) {
                        ids.push(id);
                    }
                }
                postings.push(ids);
            }
            let parts: Vec<_> = (0..1 + case % 8).rev().map(GramQuery::Gram).collect();
            let lookup = |g: u64| postings.get(g as usize).map_or(&[][..], Vec::as_slice);
            let expected: Vec<_> = (0..128)
                .filter(|id| {
                    parts.iter().all(|part| match part {
                        GramQuery::Gram(g) => lookup(*g).contains(id),
                        _ => false,
                    })
                })
                .collect();
            assert_eq!(intersect_flat(&parts, &lookup), Some(expected));
        }
    }

    #[test]
    fn skew_empty_and_nested_inputs_keep_their_contract() {
        let many: Vec<_> = (0..4096).collect();
        let rare = [4095];
        let lookup = |g| match g {
            0 => many.as_slice(),
            1 => &rare,
            _ => &[],
        };
        assert_eq!(
            intersect_flat(&[GramQuery::Gram(0), GramQuery::Gram(1)], &lookup),
            Some(vec![4095])
        );
        assert_eq!(
            intersect_flat(&[GramQuery::Gram(0), GramQuery::Gram(0)], &lookup),
            Some(many.clone())
        );
        assert_eq!(
            intersect_flat(&[GramQuery::Gram(0), GramQuery::Gram(2)], &lookup),
            Some(vec![])
        );
        assert_eq!(intersect_flat(&[], &lookup), None);
        assert_eq!(intersect_flat(&[GramQuery::All], &lookup), None);
        assert_eq!(
            intersect_flat(&[GramQuery::Or(vec![GramQuery::Gram(0)])], &lookup),
            None
        );
    }

    #[test]
    fn public_candidates_preserve_fallbacks_views_and_encoded_bytes() {
        let mut builder = NgramIndexBuilder::new(&[View::Raw, View::Decoded], 32);
        for (i, value) in [
            &b"powershell abcdef"[..],
            &b"abc xyz"[..],
            &b"abcdef xyz"[..],
            &b"%70owershell\0"[..],
            &b"\xffabcdef"[..],
            &b""[..],
            &[b'x'; 64][..],
        ]
        .iter()
        .enumerate()
        {
            let id = builder.add_file(&format!("file-{i}"));
            builder.add_value(id, value);
        }
        let index = builder.finish();
        let before = index.encode();
        for view in View::ALL {
            for (kind, pattern) in [
                (Kind::Wildcard, "*powershell*"),
                (Kind::Wildcard, "*abcdef*xyz*"),
                (Kind::Wildcard, "*absent*"),
                (Kind::Wildcard, "*"),
                (
                    Kind::Regex {
                        case_insensitive: false,
                    },
                    "(abc|xyz).*def",
                ),
            ] {
                let matcher = Matcher::new(kind, pattern, view).unwrap();
                let mut lookup = |g| {
                    index
                        .grams
                        .binary_search_by_key(&g, |e| e.0)
                        .map_or_else(|_| Vec::new(), |i| index.grams[i].1.clone())
                };
                let mut ids = if index.views.contains(&view) {
                    matcher
                        .plan
                        .candidates(index.files.len() as u32, &mut lookup)
                } else {
                    (0..index.files.len() as u32).collect()
                };
                ids.extend(
                    index
                        .files
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| f.has_unindexed)
                        .map(|(i, _)| i as u32),
                );
                ids.sort_unstable();
                ids.dedup();
                let expected: Vec<_> = ids.iter().map(|&i| &index.files[i as usize]).collect();
                assert_eq!(
                    index.candidates(kind, pattern, view).unwrap().files,
                    expected
                );
            }
        }
        assert_eq!(before, index.encode());
    }
}
