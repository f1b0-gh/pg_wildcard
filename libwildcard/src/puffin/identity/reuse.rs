// SPDX-License-Identifier: Apache-2.0
//! Reuse complete immutable-file postings, preserving every conservative fallback.
use super::*;

impl IdentityIndex {
    /// Compose complete files from authenticated immutable sources. Omitted
    /// files are dropped; unknown/duplicate paths and profile/budget mismatch
    /// are errors. Reused saturation and whole-file fallbacks stay conservative.
    pub fn assemble(&self, fresh: &Self, paths: &[String]) -> Result<Self, Error> {
        ensure(
            self.binding == fresh.binding
                && self.views == fresh.views
                && self.max_value_bytes == fresh.max_value_bytes,
            "reuse profile mismatch",
        )?;
        ensure(paths.len() <= MAX_FILES, "file budget")?;
        let mut inventory = BTreeMap::new();
        for file in self.files.iter().chain(&fresh.files) {
            ensure(
                inventory.insert(file.path.as_str(), file).is_none(),
                "duplicate source path",
            )?;
        }
        let mut unique = BTreeSet::new();
        let mut path_bytes = 0usize;
        let mut usage = Usage::default();
        // Preflight the whole selected inventory before cloning any postings.
        for path in paths {
            path_bytes = path_bytes
                .checked_add(path.len())
                .ok_or(Error::Invalid("paths"))?;
            ensure(
                !path.is_empty() && path_bytes <= MAX_PATH_BYTES && unique.insert(path),
                "paths",
            )?;
            let file = inventory
                .get(path.as_str())
                .ok_or(Error::Invalid("uncovered inventory path"))?;
            usage.add(Usage::of(file));
            ensure(
                usage.grams <= MAX_GRAMS
                    && usage.postings <= MAX_POSTINGS
                    && usage.bytes <= MAX_GUARD_BYTES,
                "combined posting budget",
            )?;
        }
        let files = paths
            .iter()
            .map(|path| {
                inventory
                    .get(path.as_str())
                    .map(|file| (*file).clone())
                    .ok_or(Error::Invalid("uncovered inventory path"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            binding: self.binding.clone(),
            views: self.views.clone(),
            max_value_bytes: self.max_value_bytes,
            files,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> Binding {
        Binding {
            table_uuid: "00000000-0000-4000-8000-000000000001".into(),
            column: "message".into(),
            guard: "event_id".into(),
            column_field_id: 1,
            guard_field_id: 2,
        }
    }
    fn index(paths: &[&str]) -> IdentityIndex {
        let mut b = Builder::new(binding(), &View::ALL, 8192).unwrap();
        for path in paths {
            b.start_file(path).unwrap();
            b.add_value(Some(b"powershell target"), Some("id'\\\0é"))
                .unwrap();
        }
        b.finish()
    }
    #[test]
    fn reuse_reorders_drops_and_appends_complete_files() {
        let old = index(&["a", "removed"]);
        let fresh = index(&["b"]);
        let result = old.assemble(&fresh, &["b".into(), "a".into()]).unwrap();
        assert_eq!(result, index(&["b", "a"]));
        assert_eq!(
            result.write(123, 2).unwrap(),
            index(&["b", "a"]).write(123, 2).unwrap()
        );
        assert_eq!(old.assemble(&index(&[]), &[]).unwrap().files().len(), 0);
    }
    #[test]
    fn reuse_preserves_saturation_and_whole_file_fallback() {
        let mut b = Builder::new(binding(), &View::ALL, 8192).unwrap();
        b.start_file("saturated").unwrap();
        for i in 0..33 {
            b.add_value(Some(b"target"), Some(&format!("id{i}")))
                .unwrap();
        }
        b.start_file("missing").unwrap();
        b.add_value(Some(b"target"), None).unwrap();
        let old = b.finish();
        let fresh = index(&["new"]);
        let result = old
            .assemble(
                &fresh,
                &["missing".into(), "new".into(), "saturated".into()],
            )
            .unwrap();
        assert_eq!(result.files[0], old.files[1]);
        assert_eq!(result.files[2], old.files[0]);
        let plan = result
            .plan(&binding(), Kind::Wildcard, "*target*", View::Raw)
            .unwrap();
        // The only other file has one row and its guard covers that row.
        // Saturated/missing guards plus this nonselective singleton need no plan.
        assert!(plan.full_scan);
    }
    #[test]
    fn reuse_refuses_profiles_duplicates_and_uncovered_inventory() {
        let old = index(&["a"]);
        let fresh = index(&["b"]);
        for paths in [
            vec!["a".into(), "a".into()],
            vec!["missing".into()],
            vec!["".into()],
        ] {
            assert!(old.assemble(&fresh, &paths).is_err());
        }
        assert!(old.assemble(&index(&["a"]), &["a".into()]).is_err());
        for changed in 0..3 {
            let mut other = fresh.clone();
            match changed {
                0 => other.binding.guard_field_id = 3,
                1 => other.views = vec![View::Raw],
                _ => other.max_value_bytes = 32,
            };
            assert!(old.assemble(&other, &["a".into(), "b".into()]).is_err());
        }
        let long = "a".repeat(33000);
        let long2 = "b".repeat(33000);
        assert!(
            index(&[&long])
                .assemble(&index(&[&long2]), &[long, long2])
                .is_err()
        );
        let paths: Vec<_> = (0..129).map(|i| format!("f{i}")).collect();
        let a: Vec<_> = paths[..65].iter().map(String::as_str).collect();
        let b: Vec<_> = paths[65..].iter().map(String::as_str).collect();
        assert!(index(&a).assemble(&index(&b), &paths).is_err());
    }
    #[test]
    fn combined_gram_posting_and_guard_byte_budgets_are_enforced() {
        fn large(path: &str, grams: usize, ids: usize, width: usize) -> IdentityIndex {
            let mut index = index(&[]);
            let values: BTreeSet<Arc<str>> =
                (0..ids).map(|n| format!("{n:0width$}").into()).collect();
            let postings = (0..grams)
                .map(|g| {
                    (
                        crate::core::grams::key(
                            View::Raw,
                            (g % 256) as u16,
                            ((g / 256) % 256) as u16,
                            0,
                        ),
                        values.clone(),
                    )
                })
                .collect();
            index.files.push(FileIndex {
                path: path.into(),
                rows: ids as u64,
                unrestricted: false,
                postings,
            });
            index
        }
        for (grams, ids, width) in [(40000, 1, 1), (20000, 32, 2), (3000, 32, 200)] {
            let a = large("a", grams, ids, width);
            let b = large("b", grams, ids, width);
            // Each source is individually accepted by the real bounded codec.
            assert_eq!(
                IdentityIndex::decode_payload(&a.encode_payload().unwrap()).unwrap(),
                a
            );
            assert!(a.assemble(&b, &["a".into(), "b".into()]).is_err());
            assert_eq!(a.assemble(&b, &["b".into()]).unwrap(), b);
        }
    }
}
