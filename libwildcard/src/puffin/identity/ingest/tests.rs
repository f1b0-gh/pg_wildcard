// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::{cell::Cell, collections::hash_map::RandomState, hash::BuildHasher, rc::Rc};

struct CountedRandomState {
    state: RandomState,
    hashes: Rc<Cell<usize>>,
}
impl BuildHasher for CountedRandomState {
    type Hasher = <RandomState as BuildHasher>::Hasher;
    fn build_hasher(&self) -> Self::Hasher {
        self.hashes.set(self.hashes.get() + 1);
        self.state.build_hasher()
    }
}
fn pending<S: BuildHasher>(state: S, capacity: usize) -> Pending<S> {
    Pending {
        path: "file".into(),
        rows: 0,
        unrestricted: false,
        grams: HashMap::with_capacity_and_hasher(capacity, state),
        usage: Usage::default(),
    }
}

#[test]
fn existing_and_saturated_grams_hash_once_per_observation() {
    let hashes = Rc::new(Cell::new(0));
    let mut p = pending(
        CountedRandomState {
            state: RandomState::new(),
            hashes: hashes.clone(),
        },
        128,
    );
    let retained = Usage::default();
    p.add_gram(42, "first", &retained);
    for id in ["first", "second", "second"] {
        hashes.set(0);
        p.add_gram(42, id, &retained);
        assert_eq!(hashes.get(), 1, "existing gram ({id})");
    }
    for i in 2..IDS_PER_GRAM {
        p.add_gram(42, &format!("id-{i}"), &retained);
    }
    hashes.set(0);
    p.add_gram(42, "saturating", &retained);
    assert_eq!(hashes.get(), 1, "saturation must update the borrowed slot");
    assert!(p.grams[&42].is_none());
    hashes.set(0);
    for _ in 0..1000 {
        p.add_gram(42, "later", &retained);
    }
    assert_eq!(
        hashes.get(),
        1000,
        "saturated keys still dominate ordinary telemetry"
    );
    assert_eq!(p.usage.postings, 0);
    assert_eq!(p.usage.bytes, 0);
}

#[test]
fn exhausted_budgets_keep_duplicates_and_saturation_but_forbid_growth() {
    let mut p = pending(RandomState::new(), 128);
    for i in 0..IDS_PER_GRAM {
        p.add_gram(42, &format!("id-{i}"), &Usage::default());
    }
    let retained = Usage {
        grams: MAX_GRAMS - 1,
        postings: MAX_POSTINGS - IDS_PER_GRAM,
        bytes: MAX_GUARD_BYTES - p.usage.bytes,
    };
    let bytes = p.usage.bytes;
    p.add_gram(42, "id-0", &retained);
    assert!(!p.unrestricted);
    assert_eq!(p.usage.postings, IDS_PER_GRAM);
    assert_eq!(p.usage.bytes, bytes);
    p.add_gram(42, "33rd", &retained);
    assert!(!p.unrestricted);
    assert_eq!(p.usage.grams, 1);
    assert_eq!(p.usage.postings, 0);
    assert_eq!(p.usage.bytes, 0);
    p.add_gram(42, "late", &retained);
    assert!(p.grams[&42].is_none());
    p.add_gram(43, "new", &retained);
    assert!(p.unrestricted);
    assert!(p.grams.is_empty());
    assert_eq!(p.usage.grams, 0);

    // A vacant Entry may reserve buckets. Reaching the global gram budget must
    // not grow even the table capacity before conservative fallback.
    let mut p = pending(RandomState::new(), 3);
    let capacity = p.grams.capacity();
    for g in 0..capacity {
        p.add_gram(g as u64, "id", &Usage::default());
    }
    assert_eq!(p.grams.capacity(), capacity);
    let retained = Usage {
        grams: MAX_GRAMS - capacity,
        ..Usage::default()
    };
    p.add_gram(u64::MAX, "new", &retained);
    assert!(p.unrestricted);
    assert_eq!(p.grams.capacity(), capacity);
}

#[test]
fn sparse_slot_keeps_only_a_key_and_optional_pointer() {
    assert_eq!(
        std::mem::size_of::<(u64, PendingIds)>(),
        std::mem::size_of::<(u64, usize)>(),
        "saturated slots must not retain an inline BTreeSet-sized payload"
    );
}
