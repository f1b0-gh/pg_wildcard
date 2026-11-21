// SPDX-License-Identifier: Apache-2.0
//! Backend-local compiled-pattern cache; no PostgreSQL pointers or row values.

use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

pub(crate) const CACHE_CAP: usize = 256;
/// Accounted retained bytes, not process RSS or an allocator-exact heap bound.
pub(crate) const CACHE_BYTES: usize = 32 << 20;

pub(crate) struct MatcherCache<K, V> {
    entries: HashMap<K, HashMap<Rc<str>, Rc<V>>>,
    recent: Option<(K, Rc<str>, Rc<V>)>,
    len: usize,
    charged_bytes: usize,
}

impl<K: Copy + Eq + Hash, V> Default for MatcherCache<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            recent: None,
            len: 0,
            charged_bytes: 0,
        }
    }
}

impl<K: Copy + Eq + Hash, V> MatcherCache<K, V> {
    pub(crate) fn get_or_compile<E>(
        &mut self,
        pattern: &str,
        key: K,
        compile: impl FnOnce() -> Result<V, E>,
        charge: impl FnOnce(&V) -> usize,
    ) -> Result<Rc<V>, E> {
        if let Some((last_key, last_pattern, value)) = &self.recent
            && *last_key == key
            && last_pattern.as_ref() == pattern
        {
            return Ok(value.clone());
        }
        if let Some((pattern, value)) = self
            .entries
            .get(&key)
            .and_then(|m| m.get_key_value(pattern))
        {
            self.recent = Some((key, pattern.clone(), value.clone()));
            return Ok(value.clone());
        }
        // Release a saturated cache before allocating the next compiled value.
        // Otherwise one in-flight compilation may overlap the retained budget.
        // Caller-held Rc aliases are outside this cache's ownership budget.
        if self.len >= CACHE_CAP || self.charged_bytes >= CACHE_BYTES {
            self.clear();
        }
        let value = Rc::new(compile()?);
        // Include key bytes and a fixed per-entry metadata allowance. The
        // value's charge documents its own scope (the regex engine is approximate).
        let charged = charge(&value)
            .saturating_add(pattern.len())
            .saturating_add(256);
        if charged > CACHE_BYTES {
            return Ok(value); // Valid large patterns still execute, without retention.
        }
        if self.charged_bytes.saturating_add(charged) > CACHE_BYTES {
            self.clear();
        }
        let pattern: Rc<str> = pattern.into();
        self.entries
            .entry(key)
            .or_default()
            .insert(pattern.clone(), value.clone());
        self.recent = Some((key, pattern, value.clone()));
        self.len += 1;
        self.charged_bytes += charged;
        Ok(value)
    }

    fn clear(&mut self) {
        self.entries = HashMap::new();
        self.recent = None;
        self.len = 0;
        self.charged_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
    }

    struct CountingAllocator;
    // SAFETY: every allocation is delegated unchanged to the system allocator;
    // the thread-local counter performs no allocations and tracks this thread only.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|n| {
                if let Some(count) = n.get() {
                    n.set(Some(count + 1));
                }
            });
            // SAFETY: same layout and allocation contract as GlobalAlloc.
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: allocation and layout are delegated without modification.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    fn get(c: &mut MatcherCache<(u8, u8), String>, p: &str, k: (u8, u8)) -> Rc<String> {
        c.get_or_compile(p, k, || Ok::<_, ()>(format!("{p}:{k:?}")), |v| v.capacity())
            .unwrap()
    }

    #[test]
    fn repeated_and_alternating_hits_allocate_nothing() {
        let mut c = MatcherCache::default();
        let a = get(&mut c, "event=*", (0, 0));
        let b = get(&mut c, "event=*", (1, 0));
        ALLOCATIONS.with(|n| n.set(Some(0)));
        for i in 0_u16..1000 {
            let k = if i < 500 || i.is_multiple_of(2) {
                (0, 0)
            } else {
                (1, 0)
            };
            let v = c
                .get_or_compile(
                    "event=*",
                    k,
                    || -> Result<String, ()> { panic!("a cached pattern must not compile again") },
                    |v| v.capacity(),
                )
                .unwrap();
            assert!(Rc::ptr_eq(&v, if k.0 == 0 { &a } else { &b }));
        }
        let allocations = ALLOCATIONS.with(|n| n.replace(None).unwrap());
        assert_eq!(allocations, 0, "cache hits must not allocate keys");
    }

    #[test]
    fn keys_distinguish_pattern_kind_and_view() {
        let mut c = MatcherCache::default();
        let a = get(&mut c, "a*", (0, 0));
        for (p, k) in [("a?", (0, 0)), ("a*", (1, 0)), ("a*", (0, 1))] {
            assert!(!Rc::ptr_eq(&a, &get(&mut c, p, k)));
            assert!(Rc::ptr_eq(&a, &get(&mut c, "a*", (0, 0))));
        }
    }

    #[test]
    fn byte_budget_evicts_before_entry_capacity_and_preserves_caller_aliases() {
        let mut c = MatcherCache::<u8, ()>::default();
        let mut get = |pattern: &str| {
            c.get_or_compile(pattern, 0, || Ok::<_, ()>(()), |_| 12 << 20)
                .unwrap()
        };
        let first = get("first");
        let second = get("second");
        let weak_second = Rc::downgrade(&second);
        drop(second);
        let third = get("third");
        assert!(weak_second.upgrade().is_none());
        assert_eq!(
            Rc::strong_count(&first),
            1,
            "caller-held values survive eviction"
        );
        assert_eq!(c.len, 1);
        assert!(c.charged_bytes <= CACHE_BYTES);
        assert!(Rc::ptr_eq(
            &third,
            &c.get_or_compile("third", 0, || -> Result<(), ()> { panic!("hit") }, |_| 0)
                .unwrap()
        ));
    }

    #[test]
    fn oversized_values_execute_without_cache_retention() {
        let mut c = MatcherCache::<u8, ()>::default();
        for size in [CACHE_BYTES, usize::MAX] {
            let value = c
                .get_or_compile("oversized", 0, || Ok::<_, ()>(()), |_| size)
                .unwrap();
            assert_eq!(Rc::strong_count(&value), 1);
            assert_eq!(c.len, 0);
            assert_eq!(c.charged_bytes, 0);
            assert!(c.recent.is_none());
        }
    }

    #[test]
    fn full_cache_releases_ownership_before_compilation() {
        let mut c = MatcherCache::default();
        let first = get(&mut c, "first", (0, 0));
        let weak = Rc::downgrade(&first);
        drop(first);
        for i in 1..CACHE_CAP {
            get(&mut c, &i.to_string(), (0, 0));
        }
        c.get_or_compile(
            "overflow",
            (0, 0),
            || {
                assert!(
                    weak.upgrade().is_none(),
                    "old compiled state overlaps compilation"
                );
                Ok::<_, ()>(String::new())
            },
            |v| v.capacity(),
        )
        .unwrap();
    }

    #[test]
    fn compile_errors_leave_cache_reusable() {
        let mut c = MatcherCache::default();
        let first = get(&mut c, "first", (0, 0));
        let weak = Rc::downgrade(&first);
        drop(first);
        for i in 1..CACHE_CAP - 1 {
            get(&mut c, &i.to_string(), (0, 0));
        }
        get(&mut c, "first", (0, 0));
        assert!(
            c.get_or_compile(
                "invalid",
                (0, 0),
                || Err::<String, _>("bad"),
                |v| v.capacity()
            )
            .is_err()
        );
        assert!(weak.upgrade().is_some());
        get(&mut c, "last", (0, 0));
        get(&mut c, "overflow", (0, 0));
        assert!(
            weak.upgrade().is_none(),
            "eviction must release the recent entry too"
        );
    }
}
