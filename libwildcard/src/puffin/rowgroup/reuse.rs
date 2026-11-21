// SPDX-License-Identifier: Apache-2.0
//! Compose trusted complete indexes for immutable paths; never re-read evidence.
use super::*;

impl GroupIndex {
    pub fn max_value_bytes(&self) -> usize {
        self.header.max_value_bytes
    }

    /// Assemble complete immutable files in inventory order. Both sources must
    /// have the same trusted binding, views and value limit. Omitted paths are
    /// removed; duplicates and paths absent from both sources are errors.
    pub fn assemble(&self, fresh: &Self, paths: &[String]) -> Result<Self, Error> {
        use std::collections::{BTreeMap, BTreeSet};
        self.validate()?;
        fresh.validate()?;
        ensure(
            self.binding() == fresh.binding()
                && self.grams.views == fresh.grams.views
                && self.max_value_bytes() == fresh.max_value_bytes(),
            "reuse profile mismatch",
        )?;
        ensure(paths.len() <= MAX_FILES, "coverage budget")?;
        let mut path_bytes = 0usize;
        let mut unique = BTreeSet::new();
        for path in paths {
            path_bytes = path_bytes
                .checked_add(path.len())
                .ok_or(Error::Invalid("paths"))?;
            ensure(
                !path.is_empty() && path_bytes <= MAX_PATH_BYTES && unique.insert(path),
                "paths",
            )?;
        }
        let sources = [self, fresh];
        let mut inventory = BTreeMap::new();
        for (source, index) in sources.iter().enumerate() {
            let mut start = 0usize;
            for (file, entry) in index.files().iter().enumerate() {
                ensure(
                    inventory
                        .insert(entry.path.as_str(), (source, file, start))
                        .is_none(),
                    "duplicate source path",
                )?;
                start += entry.groups as usize;
            }
        }
        let mut group_count = 0usize;
        for path in paths {
            let &(source, file, _) = inventory
                .get(path.as_str())
                .ok_or(Error::Invalid("uncovered inventory path"))?;
            group_count += sources[source].files()[file].groups as usize;
            ensure(group_count <= MAX_GROUPS, "group budget")?;
        }
        let mut files = Vec::with_capacity(paths.len());
        let mut groups = Vec::with_capacity(group_count);
        let mut entries = Vec::with_capacity(group_count);
        let mut remap = [
            vec![None; self.groups().len()],
            vec![None; fresh.groups().len()],
        ];
        for (file, path) in paths.iter().enumerate() {
            let &(source, old_file, start) = inventory
                .get(path.as_str())
                .ok_or(Error::Invalid("uncovered inventory path"))?;
            let index = sources[source];
            let entry = &index.files()[old_file];
            files.push(entry.clone());
            for (old_group, target) in remap[source]
                .iter_mut()
                .enumerate()
                .skip(start)
                .take(entry.groups as usize)
            {
                let id = groups.len() as u32;
                let mut group = index.groups()[old_group].clone();
                group.file = file as u32;
                groups.push(group);
                let mut value = index.grams.files[old_group].clone();
                value.path = id.to_string();
                entries.push(value);
                *target = Some(id);
            }
        }
        // Count selected postings before constructing their combined allocation.
        let mut postings = 0usize;
        for (source, index) in sources.iter().enumerate() {
            for (_, post) in &index.grams.grams {
                postings += post
                    .iter()
                    .filter(|p| remap[source][**p as usize].is_some())
                    .count();
                ensure(postings <= MAX_POSTINGS, "posting budget")?;
            }
        }
        let mut grams: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
        for (source, index) in sources.iter().enumerate() {
            for (gram, post) in &index.grams.grams {
                for id in post.iter().filter_map(|p| remap[source][*p as usize]) {
                    grams.entry(*gram).or_default().push(id);
                }
            }
        }
        for post in grams.values_mut() {
            post.sort_unstable();
            post.dedup();
        }
        Self::new(
            self.binding().clone(),
            files,
            groups,
            NgramIndex {
                views: self.grams.views.clone(),
                files: entries,
                grams: grams.into_iter().collect(),
            },
            self.max_value_bytes(),
        )
    }

    /// Exact file-level union of all group grams, retaining null row counts and
    /// the overlong-value fallback. No Parquet read or normalization occurs.
    pub fn file_index(&self) -> Result<NgramIndex, Error> {
        use crate::puffin::ngram::FileEntry;
        self.validate()?;
        let mut files: Vec<_> = self
            .files()
            .iter()
            .map(|f| FileEntry {
                path: f.path.clone(),
                record_count: 0,
                has_unindexed: false,
            })
            .collect();
        for (group, entry) in self.groups().iter().zip(&self.grams.files) {
            let file = &mut files[group.file as usize];
            file.record_count = file
                .record_count
                .checked_add(entry.record_count)
                .ok_or(Error::Invalid("file row count overflow"))?;
            file.has_unindexed |= entry.has_unindexed;
        }
        let grams = self
            .grams
            .grams
            .iter()
            .filter_map(|(gram, post)| {
                let mut files: Vec<_> = post
                    .iter()
                    .map(|g| self.groups()[*g as usize].file)
                    .collect();
                // Group ordinals are ordered by complete file coverage.
                files.dedup();
                (!files.is_empty()).then_some((*gram, files))
            })
            .collect();
        Ok(NgramIndex {
            views: self.grams.views.clone(),
            files,
            grams,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puffin::ngram::{FileEntry, NgramIndexBuilder};

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

    fn one(path: &str, values: &[&[Option<&[u8]>]], bounds: &[Option<(i64, i64)>]) -> GroupIndex {
        let mut grams = NgramIndexBuilder::new(&View::ALL, 64);
        for (i, values) in values.iter().enumerate() {
            let g = grams.add_file(&i.to_string());
            for value in *values {
                if let Some(value) = value {
                    grams.add_value(g, value);
                } else {
                    grams.add_null(g);
                }
            }
        }
        GroupIndex::new(
            binding(),
            vec![DataFile {
                path: path.into(),
                groups: values.len() as u32,
            }],
            bounds
                .iter()
                .enumerate()
                .map(|(i, b)| Group {
                    file: 0,
                    ordinal: i as u32,
                    bounds: *b,
                })
                .collect(),
            grams.finish(),
            64,
        )
        .unwrap()
    }

    #[test]
    fn assembly_remaps_and_projects_exact_full_file_grams() {
        let long = [b'x'; 65];
        let first = [Some(b"PowerShell.exe".as_slice()), None];
        let second = [
            Some(long.as_slice()),
            Some(b"C:\\Windows\\Temp\\a.exe".as_slice()),
        ];
        let third = [Some(b"https://evil.example/%2e%2e/a".as_slice())];
        let old = one("old", &[&first, &second], &[Some((1, 2)), None]);
        let fresh = one("new", &[&third], &[Some((-1, 0))]);
        let merged = old.assemble(&fresh, &["new".into(), "old".into()]).unwrap();
        assert_eq!(
            merged.groups(),
            &[
                Group {
                    file: 0,
                    ordinal: 0,
                    bounds: Some((-1, 0))
                },
                Group {
                    file: 1,
                    ordinal: 0,
                    bounds: Some((1, 2))
                },
                Group {
                    file: 1,
                    ordinal: 1,
                    bounds: None
                }
            ]
        );
        let mut expected = NgramIndexBuilder::new(&View::ALL, 64);
        let f = expected.add_file("new");
        for value in third {
            expected.add_value(f, value.unwrap());
        }
        let f = expected.add_file("old");
        for value in first.into_iter().chain(second) {
            if let Some(v) = value {
                expected.add_value(f, v);
            } else {
                expected.add_null(f);
            }
        }
        assert_eq!(
            merged.file_index().unwrap().encode(),
            expected.finish().encode()
        );
        assert_eq!(
            GroupIndex::read(&merged.write(2, 2).unwrap()).unwrap(),
            merged
        );
        let filtered = old.assemble(&fresh, &["new".into()]).unwrap();
        assert_eq!(filtered, fresh);
        assert_eq!(old.assemble(&fresh, &[]).unwrap().files().len(), 0);
    }

    #[test]
    fn empty_files_and_zero_group_paths_survive() {
        let old = one("empty", &[], &[]);
        let fresh = one("data", &[&[Some(b"needle")]], &[Some((1, 1))]);
        let joined = old
            .assemble(&fresh, &["empty".into(), "data".into()])
            .unwrap();
        assert_eq!(
            joined.file_index().unwrap().files[0],
            FileEntry {
                path: "empty".into(),
                record_count: 0,
                has_unindexed: false
            }
        );
        assert_eq!(joined.groups()[0].file, 1);
    }

    #[test]
    fn assembly_rejects_configuration_and_path_conflicts() {
        let old = one("old", &[], &[]);
        let fresh = one("new", &[], &[]);
        for mode in 0..5 {
            let mut wrong = fresh.clone();
            match mode {
                0 => {
                    wrong.header.binding.table_uuid = "00000000-0000-0000-0000-000000000002".into()
                }
                1 => wrong.header.binding.column_field_id = 3,
                2 => wrong.header.binding.guard_type = GuardType::TimestampUsUtc,
                3 => wrong.grams.views = vec![View::Raw],
                _ => wrong.header.max_value_bytes = 32,
            }
            assert!(old.assemble(&wrong, &["old".into(), "new".into()]).is_err());
        }
        assert!(old.assemble(&fresh, &["old".into(), "old".into()]).is_err());
        assert!(old.assemble(&fresh, &["unknown".into()]).is_err());
        assert!(old.assemble(&old, &["old".into()]).is_err());
    }

    fn large(path: &str, n: usize, grams: usize) -> GroupIndex {
        GroupIndex::new(
            binding(),
            vec![DataFile {
                path: path.into(),
                groups: n as u32,
            }],
            (0..n)
                .map(|i| Group {
                    file: 0,
                    ordinal: i as u32,
                    bounds: None,
                })
                .collect(),
            NgramIndex {
                views: vec![View::Raw],
                files: (0..n)
                    .map(|i| FileEntry {
                        path: i.to_string(),
                        record_count: 0,
                        has_unindexed: false,
                    })
                    .collect(),
                grams: (0..grams)
                    .map(|g| (g as u64, (0..n as u32).collect()))
                    .collect(),
            },
            64,
        )
        .unwrap()
    }

    #[test]
    fn assembly_enforces_combined_budgets() {
        let old = large("old", MAX_GROUPS, 0);
        let fresh = large("new", 1, 0);
        assert!(old.assemble(&fresh, &["old".into(), "new".into()]).is_err());
        let old = large(&"a".repeat(MAX_PATH_BYTES / 2 + 1), 0, 0);
        let fresh = large(&"b".repeat(MAX_PATH_BYTES / 2 + 1), 0, 0);
        assert!(
            old.assemble(
                &fresh,
                &[old.files()[0].path.clone(), fresh.files()[0].path.clone()]
            )
            .is_err()
        );
        let old = large("old", 2048, 245);
        let fresh = large("new", 2048, 245);
        assert!(old.assemble(&fresh, &["old".into(), "new".into()]).is_err());
        // Dropping the old file permits the same new index without carrying old postings.
        assert_eq!(old.assemble(&fresh, &["new".into()]).unwrap(), fresh);
    }

    #[test]
    fn file_projection_refuses_row_count_overflow() {
        let mut index = large("file", 2, 0);
        index.grams.files[0].record_count = u64::MAX;
        index.grams.files[1].record_count = 1;
        assert!(index.file_index().is_err());
    }
}
