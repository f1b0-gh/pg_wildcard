// SPDX-License-Identifier: Apache-2.0
//! Explicit row-group candidate ranges. Callers must authenticate the index,
//! validate the table/schema binding and pin all query branches to one Iceberg
//! metadata version. This module never treats a group ordinal as a file path.
mod reuse;

use crate::core::{View, verify::Kind};
use crate::puffin::ngram::NgramIndex;
use serde::{Deserialize, Serialize};

pub const BLOB_TYPE: &str = "wildcard-rowgroup-v1";
pub const TIMESTAMP_BLOB_TYPE: &str = "wildcard-rowgroup-v2";
/// Conservative qualified UTC envelope, not a claim about all engine versions.
pub const MIN_TIMESTAMP_US: i64 = -2_208_988_800_000_000;
pub const MAX_TIMESTAMP_US: i64 = 10_413_791_999_999_999;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardType {
    #[default]
    Int64,
    TimestampUsUtc,
}
impl GuardType {
    fn is_int64(&self) -> bool {
        *self == Self::Int64
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Int64 => "int64",
            Self::TimestampUsUtc => "timestamp_us_utc",
        }
    }
    pub fn permits(self, tick: i64) -> bool {
        self == Self::Int64 || (MIN_TIMESTAMP_US..=MAX_TIMESTAMP_US).contains(&tick)
    }
}
impl std::str::FromStr for GuardType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "int64" => Ok(Self::Int64),
            "timestamp_us_utc" => Ok(Self::TimestampUsUtc),
            _ => Err("guard type must be int64 or timestamp_us_utc".into()),
        }
    }
}
pub const MAX_FILES: usize = 128;
pub const MAX_GROUPS: usize = 4096;
pub const MAX_PATH_BYTES: usize = 65536;
pub const MAX_POSTINGS: usize = 1_000_000;
pub const MAX_VALUE_BYTES: usize = 8192;
pub const MAX_FILE_BYTES: usize = 16 << 20;
pub const MAX_BLOB_BYTES: usize = 32 << 20;
const MAX_HEADER_BYTES: usize = 2 << 20;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub table_uuid: String,
    pub column: String,
    pub guard: String,
    pub column_field_id: i32,
    pub guard_field_id: i32,
    #[serde(default, skip_serializing_if = "GuardType::is_int64")]
    pub guard_type: GuardType,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataFile {
    pub path: String,
    pub groups: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub file: u32,
    pub ordinal: u32,
    pub bounds: Option<(i64, i64)>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    binding: Binding,
    files: Vec<DataFile>,
    groups: Vec<Group>,
    max_value_bytes: usize,
}

// Decode presence together with the typed fields. A JSON Value would duplicate
// all paths and groups while the original payload and Header are still alive.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadBinding {
    table_uuid: String,
    column: String,
    guard: String,
    column_field_id: i32,
    guard_field_id: i32,
    #[serde(default, deserialize_with = "present_guard_type")]
    guard_type: Option<GuardType>,
}

fn present_guard_type<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<GuardType>, D::Error> {
    GuardType::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadHeader {
    binding: ReadBinding,
    files: Vec<DataFile>,
    groups: Vec<Group>,
    max_value_bytes: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupIndex {
    header: Header,
    grams: NgramIndex,
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid row-group index: {0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Ngram(#[from] crate::puffin::ngram::NgramError),
    #[error(transparent)]
    Puffin(#[from] crate::puffin::puffin::PuffinError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
/// None means read the entire known file; an empty vector excludes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileRanges {
    pub path: String,
    pub ranges: Option<Vec<(i64, i64)>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub files: Vec<FileRanges>,
    pub full_scan: bool,
}
#[derive(Clone, Copy)]
pub struct PlanBudget {
    pub files: usize,
    pub ranges: usize,
    pub path_bytes: usize,
}
impl Default for PlanBudget {
    fn default() -> Self {
        Self {
            files: 31,
            ranges: 128,
            path_bytes: MAX_PATH_BYTES,
        }
    }
}
impl GroupIndex {
    pub fn new(
        binding: Binding,
        files: Vec<DataFile>,
        groups: Vec<Group>,
        mut grams: NgramIndex,
        max_value_bytes: usize,
    ) -> Result<Self, Error> {
        grams.views.sort_unstable_by_key(|v| *v as u8);
        grams.views.dedup();
        let index = Self {
            header: Header {
                binding,
                files,
                groups,
                max_value_bytes,
            },
            grams,
        };
        index.validate()?;
        Ok(index)
    }
    pub fn binding(&self) -> &Binding {
        &self.header.binding
    }
    pub fn files(&self) -> &[DataFile] {
        &self.header.files
    }
    pub fn groups(&self) -> &[Group] {
        &self.header.groups
    }
    pub fn grams(&self) -> &NgramIndex {
        &self.grams
    }

    fn validate(&self) -> Result<(), Error> {
        use std::collections::BTreeSet;
        let h = &self.header;
        let b = &h.binding;
        let identifier = |v: &str| {
            !v.is_empty()
                && v.len() <= 128
                && v.bytes().enumerate().all(|(i, c)| {
                    c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                })
        };
        let uuid = b.table_uuid.as_bytes();
        ensure(
            uuid.len() == 36
                && uuid.iter().enumerate().all(|(i, c)| {
                    if [8, 13, 18, 23].contains(&i) {
                        *c == b'-'
                    } else {
                        c.is_ascii_hexdigit()
                    }
                }),
            "table UUID",
        )?;
        ensure(
            identifier(&b.column) && identifier(&b.guard) && b.column != b.guard,
            "column names",
        )?;
        ensure(
            b.column_field_id > 0 && b.guard_field_id > 0 && b.column_field_id != b.guard_field_id,
            "field IDs",
        )?;
        ensure(
            h.files.len() <= MAX_FILES && h.groups.len() <= MAX_GROUPS,
            "coverage budget",
        )?;
        ensure(
            h.max_value_bytes > 0 && h.max_value_bytes <= MAX_VALUE_BYTES,
            "value budget",
        )?;
        let mut paths = BTreeSet::new();
        let mut path_bytes = 0usize;
        let mut group = 0usize;
        for (file, f) in h.files.iter().enumerate() {
            path_bytes = path_bytes
                .checked_add(f.path.len())
                .ok_or(Error::Invalid("paths"))?;
            ensure(
                !f.path.is_empty() && path_bytes <= MAX_PATH_BYTES && paths.insert(&f.path),
                "paths",
            )?;
            ensure(f.groups as usize <= MAX_GROUPS, "file group count")?;
            for ordinal in 0..f.groups {
                let g = h
                    .groups
                    .get(group)
                    .ok_or(Error::Invalid("incomplete groups"))?;
                ensure(
                    g.file == file as u32 && g.ordinal == ordinal,
                    "group identity",
                )?;
                ensure(
                    g.bounds.is_none_or(|(lo, hi)| {
                        lo <= hi && b.guard_type.permits(lo) && b.guard_type.permits(hi)
                    }),
                    "group range",
                )?;
                group += 1;
            }
        }
        ensure(
            group == h.groups.len() && group == self.grams.files.len(),
            "group count",
        )?;
        for (i, g) in self.grams.files.iter().enumerate() {
            ensure(g.path == i.to_string(), "gram group identity")?;
        }
        ensure(
            !self.grams.views.is_empty()
                && self
                    .grams
                    .views
                    .windows(2)
                    .all(|v| (v[0] as u8) < v[1] as u8),
            "views",
        )?;
        ensure(
            self.grams.grams.len() <= MAX_POSTINGS
                && self.grams.grams.windows(2).all(|g| g[0].0 < g[1].0),
            "grams",
        )?;
        let mut postings = 0usize;
        for (_, p) in &self.grams.grams {
            postings = postings
                .checked_add(p.len())
                .ok_or(Error::Invalid("postings"))?;
            ensure(
                postings <= MAX_POSTINGS
                    && p.windows(2).all(|v| v[0] < v[1])
                    && p.iter().all(|v| (*v as usize) < group),
                "postings",
            )?;
        }
        Ok(())
    }
    pub fn plan(
        &self,
        binding: &Binding,
        kind: Kind,
        pattern: &str,
        view: View,
        budget: PlanBudget,
    ) -> Result<Plan, Error> {
        ensure(
            binding == &self.header.binding,
            "table/schema binding mismatch",
        )?;
        let candidates = self.grams.candidates(kind, pattern, view)?;
        let scan = || Plan {
            files: Vec::new(),
            full_scan: true,
        };
        if !candidates.used_index || self.header.files.len() > budget.files.min(31) {
            return Ok(scan());
        }
        let mut selected = vec![false; self.header.groups.len()];
        for c in candidates.files {
            let i = c
                .path
                .parse::<usize>()
                .map_err(|_| Error::Invalid("group ordinal"))?;
            *selected.get_mut(i).ok_or(Error::Invalid("group ordinal"))? = true;
        }
        let mut files: Vec<FileRanges> = self
            .header
            .files
            .iter()
            .map(|f| FileRanges {
                path: f.path.clone(),
                ranges: Some(Vec::new()),
            })
            .collect();
        for (g, chosen) in self.header.groups.iter().zip(selected) {
            if chosen {
                let f = &mut files[g.file as usize];
                match g.bounds {
                    None => f.ranges = None,
                    Some(range) => {
                        if let Some(r) = &mut f.ranges {
                            r.push(range);
                        }
                    }
                }
            }
        }
        let mut n = 0usize;
        let mut bytes = 0usize;
        for f in &mut files {
            bytes += f.path.len();
            if let Some(ranges) = &mut f.ranges {
                ranges.sort_unstable();
                let mut merged: Vec<(i64, i64)> = Vec::new();
                for &(lo, hi) in ranges.iter() {
                    if let Some(last) = merged.last_mut()
                        && lo <= last.1.saturating_add(1)
                    {
                        last.1 = last.1.max(hi);
                        continue;
                    }
                    merged.push((lo, hi));
                }
                n += merged.len();
                *ranges = merged;
            }
        }
        if n > budget.ranges.min(128) || bytes > budget.path_bytes.min(MAX_PATH_BYTES) {
            return Ok(scan());
        }
        Ok(Plan {
            files,
            full_scan: false,
        })
    }
    pub fn write(&self, snapshot_id: i64, sequence_number: i64) -> Result<Vec<u8>, Error> {
        use crate::puffin::puffin::{Blob, Codec, PuffinWriter};
        use std::collections::BTreeMap;
        self.validate()?;
        let header = serde_json::to_vec(&self.header)?;
        ensure(header.len() <= MAX_HEADER_BYTES, "header size")?;
        let timestamp = self.binding().guard_type == GuardType::TimestampUsUtc;
        let mut data = b"WCRG".to_vec();
        data.push(if timestamp { 2 } else { 1 });
        data.extend_from_slice(&(header.len() as u32).to_le_bytes());
        data.extend_from_slice(&header);
        data.extend_from_slice(&self.grams.encode());
        ensure(data.len() <= MAX_BLOB_BYTES, "blob size")?;
        let mut writer = PuffinWriter::new();
        let b = &self.header.binding;
        writer.add(
            Blob {
                blob_type: if timestamp {
                    TIMESTAMP_BLOB_TYPE
                } else {
                    BLOB_TYPE
                }
                .into(),
                fields: vec![b.column_field_id, b.guard_field_id],
                snapshot_id,
                sequence_number,
                properties: BTreeMap::from([
                    ("column".into(), b.column.clone()),
                    ("guard".into(), b.guard.clone()),
                    ("table-uuid".into(), b.table_uuid.clone()),
                ]),
                data,
            },
            Codec::Zstd,
        )?;
        let out = writer.finish(
            BTreeMap::from([("created-by".into(), "pg_wildcard".into())]),
            false,
        )?;
        ensure(out.len() <= MAX_FILE_BYTES, "Puffin size")?;
        Ok(out)
    }
    pub fn read(file: &[u8]) -> Result<Self, Error> {
        use crate::puffin::puffin::{read_blob_limited, read_footer};
        ensure(file.len() <= MAX_FILE_BYTES, "Puffin size")?;
        let footer = read_footer(file)?;
        ensure(footer.blobs.len() == 1, "expected own single-blob Puffin")?;
        let blob = &footer.blobs[0];
        let timestamp = blob.blob_type == TIMESTAMP_BLOB_TYPE;
        ensure(
            timestamp || blob.blob_type == BLOB_TYPE,
            "blob type/version",
        )?;
        let data = read_blob_limited(file, blob, MAX_BLOB_BYTES)?;
        ensure(
            data.len() >= 9
                && data.get(..4) == Some(b"WCRG")
                && data[4] == if timestamp { 2 } else { 1 },
            "magic/version",
        )?;
        let len = u32::from_le_bytes(
            data[5..9]
                .try_into()
                .map_err(|_| Error::Invalid("length"))?,
        ) as usize;
        ensure(
            len <= MAX_HEADER_BYTES && len <= data.len() - 9,
            "header length",
        )?;
        let read_header: ReadHeader =
            serde_json::from_slice(&data[9..9 + len]).map_err(|_| Error::Invalid("header JSON"))?;
        let ReadHeader {
            binding: b,
            files,
            groups,
            max_value_bytes,
        } = read_header;
        ensure(b.guard_type.is_some() == timestamp, "guard field/version")?;
        let header = Header {
            binding: Binding {
                table_uuid: b.table_uuid,
                column: b.column,
                guard: b.guard,
                column_field_id: b.column_field_id,
                guard_field_id: b.guard_field_id,
                guard_type: b.guard_type.unwrap_or_default(),
            },
            files,
            groups,
            max_value_bytes,
        };
        ensure(
            (header.binding.guard_type == GuardType::TimestampUsUtc) == timestamp,
            "guard kind/version",
        )?;
        // Header coverage is bounded before decoding the larger gram payload.
        ensure(
            header.files.len() <= MAX_FILES && header.groups.len() <= MAX_GROUPS,
            "coverage budget",
        )?;
        let encoded = &data[9 + len..];
        let grams = NgramIndex::decode_canonical_with_limits(
            encoded,
            MAX_GROUPS,
            MAX_POSTINGS,
            MAX_POSTINGS,
        )?;
        let result = Self { header, grams };
        result.validate()?;
        let b = result.binding();
        ensure(
            blob.fields == [b.column_field_id, b.guard_field_id]
                && blob.properties.get("column") == Some(&b.column)
                && blob.properties.get("guard") == Some(&b.guard)
                && blob.properties.get("table-uuid") == Some(&b.table_uuid),
            "footer binding",
        )?;
        Ok(result)
    }
}
fn ensure(ok: bool, why: &'static str) -> Result<(), Error> {
    if ok { Ok(()) } else { Err(Error::Invalid(why)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puffin::ngram::NgramIndexBuilder;
    fn binding() -> Binding {
        Binding {
            table_uuid: "00000000-0000-0000-0000-000000000001".into(),
            column: "message".into(),
            guard: "id".into(),
            column_field_id: 2,
            guard_field_id: 1,
            guard_type: GuardType::Int64,
        }
    }
    fn fixture() -> GroupIndex {
        let mut b = NgramIndexBuilder::new(&[View::Raw], MAX_VALUE_BYTES);
        for (i, v) in [b"benign".as_slice(), b"powershell.exe", b"other"]
            .iter()
            .enumerate()
        {
            let id = b.add_file(&i.to_string());
            b.add_value(id, v);
        }
        GroupIndex::new(
            binding(),
            vec![DataFile {
                path: "file'a".into(),
                groups: 3,
            }],
            (0..3)
                .map(|i| Group {
                    file: 0,
                    ordinal: i,
                    bounds: Some((i as i64 * 10, i as i64 * 10 + 9)),
                })
                .collect(),
            b.finish(),
            MAX_VALUE_BYTES,
        )
        .unwrap()
    }
    #[test]
    fn selects_candidate_group_ranges_and_rejects_wrong_binding() {
        let idx = fixture();
        let b = binding();
        let p = idx
            .plan(
                &b,
                Kind::Wildcard,
                "*powershell*",
                View::Raw,
                PlanBudget::default(),
            )
            .unwrap();
        assert!(!p.full_scan);
        assert_eq!(p.files[0].ranges, Some(vec![(10, 19)]));
        let mut wrong = b;
        wrong.guard_field_id = 3;
        assert!(
            idx.plan(
                &wrong,
                Kind::Wildcard,
                "*powershell*",
                View::Raw,
                PlanBudget::default()
            )
            .is_err()
        );
    }
    #[test]
    fn budgets_widen_and_unknown_bounds_keep_entire_file() {
        let mut idx = fixture();
        let b = binding();
        for budget in [
            PlanBudget {
                files: 0,
                ..PlanBudget::default()
            },
            PlanBudget {
                ranges: 0,
                ..PlanBudget::default()
            },
            PlanBudget {
                path_bytes: 0,
                ..PlanBudget::default()
            },
        ] {
            assert!(
                idx.plan(&b, Kind::Wildcard, "*powershell*", View::Raw, budget)
                    .unwrap()
                    .full_scan
            );
        }
        assert!(
            idx.plan(&b, Kind::Wildcard, "*", View::Raw, PlanBudget::default())
                .unwrap()
                .full_scan
        );
        assert!(
            idx.plan(
                &b,
                Kind::Wildcard,
                "*powershell*",
                View::WinPath,
                PlanBudget::default()
            )
            .unwrap()
            .full_scan
        );
        let none = idx
            .plan(
                &b,
                Kind::Wildcard,
                "*absent*",
                View::Raw,
                PlanBudget::default(),
            )
            .unwrap();
        assert_eq!(none.files[0].ranges, Some(vec![])); // Only this known file is excluded.
        idx.header.groups[1].bounds = None;
        assert_eq!(
            idx.plan(
                &b,
                Kind::Wildcard,
                "*powershell*",
                View::Raw,
                PlanBudget::default()
            )
            .unwrap()
            .files[0]
                .ranges,
            None
        );
    }
    #[test]
    fn overlapping_ranges_merge_at_signed_extrema_without_overflow() {
        let mut idx = fixture();
        let b = binding();
        for f in &mut idx.grams.files {
            f.has_unindexed = true;
        }
        idx.header.groups[0].bounds = Some((i64::MIN, 0));
        idx.header.groups[1].bounds = Some((0, i64::MAX));
        idx.header.groups[2].bounds = Some((i64::MAX, i64::MAX));
        assert_eq!(
            idx.plan(
                &b,
                Kind::Wildcard,
                "*absent*",
                View::Raw,
                PlanBudget::default()
            )
            .unwrap()
            .files[0]
                .ranges,
            Some(vec![(i64::MIN, i64::MAX)])
        );
    }
    fn repack(
        original: &[u8],
        change: impl FnOnce(&mut Vec<u8>, &mut crate::puffin::puffin::BlobMetadata),
    ) -> Vec<u8> {
        use crate::puffin::puffin::*;
        use std::collections::BTreeMap;
        let mut meta = read_footer(original).unwrap().blobs.remove(0);
        let mut data = read_blob(original, &meta).unwrap();
        change(&mut data, &mut meta);
        let mut writer = PuffinWriter::new();
        writer
            .add(
                Blob {
                    blob_type: meta.blob_type,
                    fields: meta.fields,
                    snapshot_id: meta.snapshot_id,
                    sequence_number: meta.sequence_number,
                    properties: meta.properties,
                    data,
                },
                Codec::Zstd,
            )
            .unwrap();
        writer.finish(BTreeMap::new(), false).unwrap()
    }
    #[test]
    fn rejects_truncated_wrong_version_and_inconsistent_coverage() {
        let index = fixture();
        let encoded = index.write(10, 3).unwrap();
        assert_eq!(GroupIndex::read(&encoded).unwrap(), index);
        for end in 0..encoded.len() {
            assert!(
                GroupIndex::read(&encoded[..end]).is_err(),
                "accepted truncation {end}"
            );
        }
        for mutation in 0..10 {
            let corrupt = repack(&encoded, |data, meta| match mutation {
                0 => data[4] = 2,
                1 => data[5..9].copy_from_slice(&u32::MAX.to_le_bytes()),
                2 => meta.blob_type = "wildcard-rowgroup-v2".into(),
                3 => meta.fields[0] = 999,
                4 => data.push(0),
                _ => {
                    let len = u32::from_le_bytes(data[5..9].try_into().unwrap()) as usize;
                    let mut h: serde_json::Value =
                        serde_json::from_slice(&data[9..9 + len]).unwrap();
                    match mutation {
                        5 => h["groups"][1]["ordinal"] = serde_json::json!(0),
                        6 => h["groups"][1]["bounds"] = serde_json::json!([20, 19]),
                        7 => h["files"][0]["groups"] = serde_json::json!(4),
                        8 => h["binding"]["guard_field_id"] = serde_json::json!(0),
                        _ => h["extra"] = serde_json::json!(true),
                    }
                    let header = serde_json::to_vec(&h).unwrap();
                    let tail = data[9 + len..].to_vec();
                    data.truncate(5);
                    data.extend_from_slice(&(header.len() as u32).to_le_bytes());
                    data.extend(header);
                    data.extend(tail);
                }
            });
            assert!(
                GroupIndex::read(&corrupt).is_err(),
                "accepted mutation {mutation}"
            );
        }
    }
    #[test]
    fn decoded_plans_never_drop_cyber_matches_across_views_and_groups() {
        use crate::core::verify::Matcher;
        let values: [&[u8]; 12] = [
            b"powershell.exe",
            b"C:\\Windows\\a.exe",
            b"https://EVIL.test/%2e%2e/x",
            b"",
            b"a\0b",
            b"x\xffz",
            b"scanner sqlmap",
            b"benign",
            b"abcdgh",
            b"abefgh",
            b"run2.exe",
            b"verylongunindexedcommandpowershell.exe",
        ];
        let mut b = NgramIndexBuilder::new(&View::ALL, 24);
        let mut groups = vec![];
        let mut truth = vec![];
        let mut seed = 42u64;
        for group in 0..16u32 {
            let f = b.add_file(&group.to_string());
            let mut lo = i64::MAX;
            let mut hi = i64::MIN;
            for _ in 0..11 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let id = (seed % 2000) as i64 - 1000;
                let value = values[(seed as usize >> 12) % values.len()];
                lo = lo.min(id);
                hi = hi.max(id);
                b.add_value(f, value);
                truth.push((id, value));
            }
            groups.push(Group {
                file: 0,
                ordinal: group,
                bounds: Some((lo, hi)),
            });
        }
        let index = GroupIndex::new(
            binding(),
            vec![DataFile {
                path: "known".into(),
                groups: 16,
            }],
            groups,
            b.finish(),
            24,
        )
        .unwrap();
        let index = GroupIndex::read(&index.write(1, 1).unwrap()).unwrap();
        for view in View::ALL {
            for (kind, pattern) in [
                (Kind::Wildcard, "*powershell*"),
                (Kind::Wildcard, "*windows*"),
                (Kind::Wildcard, "*"),
                (
                    Kind::Regex {
                        case_insensitive: false,
                    },
                    "ab(cd|ef)gh",
                ),
                (
                    Kind::Regex {
                        case_insensitive: false,
                    },
                    "run[0-3][.]exe",
                ),
                (Kind::Like, "%sqlmap%"),
                (Kind::Wildcard, "*%2e*"),
            ] {
                let matcher = Matcher::new(kind, pattern, view).unwrap();
                let plan = index
                    .plan(&binding(), kind, pattern, view, PlanBudget::default())
                    .unwrap();
                for &(id, value) in &truth {
                    if matcher.is_match(value) {
                        assert!(
                            plan.full_scan
                                || plan.files[0].ranges.as_ref().is_none_or(|ranges| ranges
                                    .iter()
                                    .any(|&(lo, hi)| lo <= id && id <= hi)),
                            "lost {id} {pattern} {view:?}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn timestamp_kind_requires_distinct_wire_version_and_binding() {
        let mut idx = fixture();
        idx.header.binding.guard_type = GuardType::TimestampUsUtc;
        let bytes = idx.write(1, 1).unwrap();
        let footer = crate::puffin::puffin::read_footer(&bytes).unwrap();
        assert_eq!(footer.blobs[0].blob_type, TIMESTAMP_BLOB_TYPE);
        assert_eq!(GroupIndex::read(&bytes).unwrap(), idx);
        let wrong = binding();
        assert!(
            idx.plan(
                &wrong,
                Kind::Wildcard,
                "*powershell*",
                View::Raw,
                PlanBudget::default()
            )
            .is_err()
        );
        assert!(
            GroupIndex::read(&repack(&bytes, |_, meta| {
                meta.blob_type = BLOB_TYPE.into();
            }))
            .is_err()
        );
        let v1 = fixture().write(1, 1).unwrap();
        let data = crate::puffin::puffin::read_blob(
            &v1,
            &crate::puffin::puffin::read_footer(&v1).unwrap().blobs[0],
        )
        .unwrap();
        let n = u32::from_le_bytes(data[5..9].try_into().unwrap()) as usize;
        assert!(
            !std::str::from_utf8(&data[9..9 + n])
                .unwrap()
                .contains("guard_type")
        );
        assert_eq!(
            GroupIndex::read(&v1).unwrap().binding().guard_type,
            GuardType::Int64
        );
    }
    #[test]
    fn previously_generated_v1_is_readable_and_byte_identical_on_reencode() {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/legacy-v1.puffin"
        ));
        let idx = GroupIndex::read(bytes).unwrap();
        let meta = crate::puffin::puffin::read_footer(bytes).unwrap();
        assert_eq!(idx.binding().guard_type, GuardType::Int64);
        assert_eq!(
            idx.write(meta.blobs[0].snapshot_id, meta.blobs[0].sequence_number)
                .unwrap(),
            bytes
        );
    }
}
