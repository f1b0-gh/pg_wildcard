// SPDX-License-Identifier: Apache-2.0
//! Sparse complete gram-to-identity postings. Missing/saturated grams are ALL,
//! never empty. Consumers must authenticate binding, preserve unknown files,
//! scope, snapshot/delete semantics and exact matching. IDs need not be unique.
mod codec;
#[cfg(test)]
mod dictionary_tests;
mod ingest;
mod reuse;
#[cfg(test)]
mod tests;
use crate::core::{
    View,
    query::GramQuery,
    verify::{Kind, Matcher},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

pub const LEGACY_BLOB_TYPE: &str = "wildcard-sparse-identity-v1";
pub const BLOB_TYPE: &str = "wildcard-sparse-identity-v2";
pub const MAX_FILES: usize = 128;
pub const MAX_PATH_BYTES: usize = 65536;
pub const MAX_GRAMS: usize = 65536;
pub const MAX_POSTINGS: usize = 1_000_000;
pub const MAX_GUARD_BYTES: usize = 32 << 20;
pub const MAX_VALUE_BYTES: usize = 8192;
pub const MAX_ID_BYTES: usize = 256;
pub const IDS_PER_GRAM: usize = 32;
pub const MAX_PLAN_IDS: usize = 128;
pub const MAX_FILE_BYTES: usize = 16 << 20;
pub const MAX_BLOB_BYTES: usize = 32 << 20;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid sparse identity index: {0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Puffin(#[from] crate::puffin::puffin::PuffinError),
    #[error(transparent)]
    Pattern(#[from] crate::core::verify::MatchError),
}
fn ensure(ok: bool, why: &'static str) -> Result<(), Error> {
    if ok { Ok(()) } else { Err(Error::Invalid(why)) }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub table_uuid: String,
    pub column: String,
    pub guard: String,
    pub column_field_id: i32,
    pub guard_field_id: i32,
}
impl Binding {
    fn validate(&self) -> Result<(), Error> {
        let ident = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes().enumerate().all(|(i, b)| {
                    b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit())
                })
        };
        let uuid = self.table_uuid.as_bytes();
        ensure(
            uuid.len() == 36
                && uuid.iter().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        *b == b'-'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                }),
            "table UUID",
        )?;
        ensure(
            ident(&self.column) && ident(&self.guard) && self.column != self.guard,
            "column names",
        )?;
        ensure(
            self.column_field_id > 0
                && self.guard_field_id > 0
                && self.column_field_id != self.guard_field_id,
            "field IDs",
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIndex {
    pub(crate) path: String,
    pub(crate) rows: u64,
    pub(crate) unrestricted: bool,
    pub(crate) postings: BTreeMap<u64, BTreeSet<Arc<str>>>,
}
impl FileIndex {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn rows(&self) -> u64 {
        self.rows
    }
    pub fn unrestricted(&self) -> bool {
        self.unrestricted
    }
    pub fn grams(&self) -> usize {
        self.postings.len()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityIndex {
    binding: Binding,
    views: Vec<View>,
    max_value_bytes: usize,
    files: Vec<FileIndex>,
}
#[derive(Debug, Serialize)]
pub struct Plan {
    pub files: Vec<FileCandidates>,
    pub full_scan: bool,
}
#[derive(Debug, Serialize)]
pub struct FileCandidates {
    pub path: String,
    pub values: Option<Vec<String>>,
}

/// Budget includes completed files and current postings; saturated keys retain
/// a tombstone until file completion, so an incomplete set cannot reappear.
pub struct Builder {
    index: IdentityIndex,
    current: Option<Pending>,
    retained: Usage,
}
#[derive(Clone, Copy, Default)]
struct Usage {
    grams: usize,
    postings: usize,
    bytes: usize,
}
impl Usage {
    fn of(file: &FileIndex) -> Self {
        Self {
            grams: file.postings.len(),
            postings: file.postings.values().map(BTreeSet::len).sum(),
            bytes: file
                .postings
                .values()
                .flat_map(|s| s.iter())
                .map(|id| id.len())
                .sum(),
        }
    }
    fn add(&mut self, v: Self) {
        self.grams += v.grams;
        self.postings += v.postings;
        self.bytes += v.bytes;
    }
}
// Keep saturated slots compact; live sets pay a separate bounded allocation.
type PendingIds = Option<Box<BTreeSet<Arc<str>>>>;
struct Pending<S = std::collections::hash_map::RandomState> {
    path: String,
    rows: u64,
    unrestricted: bool,
    grams: HashMap<u64, PendingIds, S>,
    usage: Usage,
}
impl<S: std::hash::BuildHasher> Pending<S> {
    fn widen(&mut self) {
        self.unrestricted = true;
        self.grams.clear();
        self.usage = Usage::default();
    }
    fn finish(self) -> FileIndex {
        FileIndex {
            path: self.path,
            rows: self.rows,
            unrestricted: self.unrestricted,
            postings: self
                .grams
                .into_iter()
                .filter_map(|(g, p)| p.map(|s| (g, *s)))
                .collect(),
        }
    }
}
impl Builder {
    pub fn new(binding: Binding, views: &[View], max_value_bytes: usize) -> Result<Self, Error> {
        binding.validate()?;
        ensure(
            (1..=MAX_VALUE_BYTES).contains(&max_value_bytes),
            "value budget",
        )?;
        let mut views = views.to_vec();
        views.sort_unstable();
        views.dedup();
        ensure(!views.is_empty(), "empty views")?;
        Ok(Self {
            index: IdentityIndex {
                binding,
                views,
                max_value_bytes,
                files: vec![],
            },
            current: None,
            retained: Usage::default(),
        })
    }
    fn finish_current(&mut self) {
        if let Some(p) = self.current.take() {
            let file = p.finish();
            self.retained.add(Usage::of(&file));
            self.index.files.push(file);
        }
    }
    pub fn start_file(&mut self, path: &str) -> Result<(), Error> {
        ensure(
            !path.is_empty() && path.len() <= MAX_PATH_BYTES,
            "file path",
        )?;
        ensure(
            self.index.files.len() + usize::from(self.current.is_some()) < MAX_FILES,
            "file budget",
        )?;
        ensure(
            !self.index.files.iter().any(|f| f.path == path)
                && !self.current.as_ref().is_some_and(|f| f.path == path),
            "duplicate file",
        )?;
        let used = self.index.files.iter().map(|f| f.path.len()).sum::<usize>()
            + self.current.as_ref().map_or(0, |f| f.path.len());
        ensure(path.len() <= MAX_PATH_BYTES - used, "path budget")?;
        self.finish_current();
        self.current = Some(Pending {
            path: path.into(),
            rows: 0,
            unrestricted: false,
            grams: HashMap::new(),
            usage: Usage::default(),
        });
        Ok(())
    }
    pub fn finish(mut self) -> IdentityIndex {
        self.finish_current();
        self.index
    }
}
impl IdentityIndex {
    pub fn binding(&self) -> &Binding {
        &self.binding
    }
    pub fn views(&self) -> &[View] {
        &self.views
    }
    pub fn files(&self) -> &[FileIndex] {
        &self.files
    }
    pub fn max_value_bytes(&self) -> usize {
        self.max_value_bytes
    }
    pub fn plan(
        &self,
        expected: &Binding,
        kind: Kind,
        pattern: &str,
        view: View,
    ) -> Result<Plan, Error> {
        ensure(expected == &self.binding, "binding mismatch")?;
        ensure(pattern.len() <= 4096, "pattern budget")?;
        let matcher = Matcher::new(kind, pattern, view)?;
        let scan = || Plan {
            files: vec![],
            full_scan: true,
        };
        if !self.views.contains(&view) || self.files.len() > 31 {
            return Ok(scan());
        }
        let mut files = vec![];
        let mut total = 0;
        let mut useful = false;
        for file in &self.files {
            let values = if file.unrestricted {
                None
            } else {
                evaluate(&matcher.plan, &file.postings).filter(|ids| {
                    // Each distinct candidate occurs in at least one indexed row.
                    // Covering every row cannot narrow this file; widen before
                    // charging the plan budget. Empty sets still exclude files.
                    ids.is_empty() || (ids.len() as u64) < file.rows
                })
            };
            if let Some(ids) = &values {
                total += ids.len();
                useful = true;
            }
            if total > MAX_PLAN_IDS {
                return Ok(scan());
            }
            files.push(FileCandidates {
                path: file.path.clone(),
                values: values.map(|s| s.into_iter().map(|id| id.to_string()).collect()),
            });
        }
        Ok(if useful {
            Plan {
                files,
                full_scan: false,
            }
        } else {
            scan()
        })
    }
}
fn evaluate(
    q: &GramQuery,
    postings: &BTreeMap<u64, BTreeSet<Arc<str>>>,
) -> Option<BTreeSet<Arc<str>>> {
    match q {
        GramQuery::All => None,
        GramQuery::Gram(g) => postings.get(g).cloned(),
        GramQuery::And(parts) => {
            let mut result: Option<BTreeSet<Arc<str>>> = None;
            for p in parts {
                if let Some(ids) = evaluate(p, postings) {
                    result = Some(match result {
                        None => ids,
                        Some(prior) => prior.intersection(&ids).cloned().collect(),
                    });
                }
            }
            result
        }
        GramQuery::Or(parts) => {
            if parts.is_empty() {
                return None;
            }
            let mut result = BTreeSet::new();
            for p in parts {
                result.extend(evaluate(p, postings)?);
                if result.len() > MAX_PLAN_IDS {
                    return None;
                }
            }
            Some(result)
        }
    }
}
