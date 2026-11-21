// SPDX-License-Identifier: Apache-2.0
//! Bounded distinct postings for one complete Parquet row group.
use super::Error;
use crate::core::grams::GramKey;
use std::collections::HashSet;

pub(super) struct PendingGrams {
    seen: HashSet<GramKey>,
    remaining: usize,
}

impl PendingGrams {
    pub(super) fn new(remaining: usize) -> Self {
        Self {
            seen: HashSet::new(),
            remaining,
        }
    }
    pub(super) fn insert(&mut self, gram: GramKey) -> Result<(), Error> {
        if !self.seen.contains(&gram) {
            if self.seen.len() >= self.remaining {
                return Err(Error::Invalid("posting budget"));
            }
            self.seen.insert(gram);
        }
        Ok(())
    }
    pub(super) fn into_iter(self) -> impl Iterator<Item = GramKey> {
        // Insert into the global BTreeMap in sorted order. Sorting once per
        // group preserves locality for high-cardinality data without sorting
        // every individual value (or revisiting existing group postings).
        let mut grams: Vec<_> = self.seen.into_iter().collect();
        grams.sort_unstable();
        grams.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{View, grams::value_grams};
    use std::collections::BTreeMap;

    #[test]
    fn pending_flush_has_sorted_tree_insertion_order() {
        let mut pending = PendingGrams::new(300);
        for key in (0..300).rev() {
            pending.insert(key).unwrap();
        }
        assert_eq!(
            pending.into_iter().collect::<Vec<_>>(),
            (0..300).collect::<Vec<_>>()
        );
    }

    #[test]
    fn pending_postings_enforce_unique_budget_before_growth() {
        let mut pending = PendingGrams::new(2);
        pending.insert(7).unwrap();
        pending.insert(9).unwrap();
        for _ in 0..1000 {
            pending.insert(7).unwrap();
        }
        assert_eq!(pending.seen.len(), 2);
        assert!(pending.insert(11).is_err());
        assert_eq!(pending.seen.len(), 2);
        let mut empty = PendingGrams::new(0);
        assert!(empty.insert(1).is_err());
        assert!(empty.seen.is_empty());
    }

    #[test]
    fn pending_groups_equal_legacy_postings_with_deterministic_output() {
        let rows: Vec<Vec<Vec<u8>>> = vec![
            vec![
                b"aaa".to_vec(),
                b"aaa".to_vec(),
                vec![0, 255, 0],
                b"%41%4%41".to_vec(),
            ],
            vec![],
            vec![
                b"aaa".to_vec(),
                b"C:\\Windows\\x.exe".to_vec(),
                vec![0, 255, 0],
            ],
        ];
        let mut legacy: BTreeMap<GramKey, Vec<u32>> = BTreeMap::new();
        for (group, values) in rows.iter().enumerate() {
            for value in values {
                for view in View::ALL {
                    for gram in value_grams(view, &view.apply(value)) {
                        let post = legacy.entry(gram).or_default();
                        if post.last() != Some(&(group as u32)) {
                            post.push(group as u32);
                        }
                    }
                }
            }
        }
        let budget: usize = legacy.values().map(Vec::len).sum();
        for _ in 0..8 {
            let mut actual: BTreeMap<GramKey, Vec<u32>> = BTreeMap::new();
            let mut count = 0;
            for (group, values) in rows.iter().enumerate() {
                let mut pending = PendingGrams::new(budget - count);
                for value in values {
                    for view in View::ALL {
                        for gram in value_grams(view, &view.apply(value)) {
                            pending.insert(gram).unwrap();
                        }
                    }
                }
                for gram in pending.into_iter() {
                    actual.entry(gram).or_default().push(group as u32);
                    count += 1;
                }
            }
            assert_eq!(count, budget);
            assert_eq!(actual, legacy);
        }
    }
}
