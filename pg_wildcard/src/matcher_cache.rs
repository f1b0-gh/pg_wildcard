// SPDX-License-Identifier: Apache-2.0
//! Backend-local compiled-pattern cache; no PostgreSQL pointers or row values.

use std::collections::{HashMap, hash_map::RandomState};
use std::hash::{BuildHasher, Hash};
use std::rc::Rc;

pub(crate) const CACHE_CAP: usize = 256;
/// Accounted retained bytes, not process RSS or an allocator-exact heap bound.
pub(crate) const CACHE_BYTES: usize = 32 << 20;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub entries: usize,
    pub charged_bytes: usize,
}

struct Entry<K, V> {
    key: K,
    pattern: Rc<str>,
    value: Rc<V>,
    charged: usize,
}

pub(crate) struct MatcherCache<K, V> {
    entries: HashMap<K, HashMap<Rc<str>, usize>>,
    slots: Vec<Entry<K, V>>,
    recent: Option<(K, Rc<str>, Rc<V>)>,
    charged_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
    random: u64,
    counters: CacheStats,
}

impl<K: Copy + Eq + Hash, V> Default for MatcherCache<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            slots: Vec::new(),
            recent: None,
            charged_bytes: 0,
            max_entries: CACHE_CAP,
            max_bytes: CACHE_BYTES,
            // Replacement is deliberately independent of access order. Seed per
            // backend; this PRNG is for eviction, not cryptographic purposes.
            random: RandomState::new().hash_one("matcher eviction") | 1,
            counters: CacheStats::default(),
        }
    }
}

impl<K: Copy + Eq + Hash, V> MatcherCache<K, V> {
    /// Apply limits immediately. Either zero limit disables retention. Existing
    /// caller-held Rc aliases are outside this cache's ownership budget.
    pub(crate) fn set_limits(&mut self, max_entries: usize, max_bytes: usize) {
        if (self.max_entries, self.max_bytes) == (max_entries, max_bytes) {
            return;
        }
        self.max_entries = max_entries;
        self.max_bytes = max_bytes;
        while self.slots.len() > max_entries || self.charged_bytes > max_bytes {
            self.evict_random();
        }
        // A reduced limit must also release historical container capacity.
        self.slots.shrink_to_fit();
        for patterns in self.entries.values_mut() {
            patterns.shrink_to_fit();
        }
        self.entries.shrink_to_fit();
    }

    pub(crate) fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.slots.len(),
            charged_bytes: self.charged_bytes,
            ..self.counters
        }
    }

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
            self.counters.hits = self.counters.hits.saturating_add(1);
            return Ok(value.clone());
        }
        if let Some(&slot) = self.entries.get(&key).and_then(|m| m.get(pattern)) {
            let entry = &self.slots[slot];
            self.recent = Some((key, entry.pattern.clone(), entry.value.clone()));
            self.counters.hits = self.counters.hits.saturating_add(1);
            return Ok(entry.value.clone());
        }
        self.counters.misses = self.counters.misses.saturating_add(1);
        // Free one slot before compiling at saturation, including on errors.
        // Compilation temporaries and the returned value are not retained cache
        // ownership. The exact charge is known only after compilation succeeds.
        if !self.slots.is_empty()
            && (self.slots.len() >= self.max_entries || self.charged_bytes >= self.max_bytes)
        {
            self.evict_random();
        }
        let value = Rc::new(compile()?);
        // Include key bytes and a fixed metadata allowance (not allocator-exact).
        let charged = charge(&value)
            .saturating_add(pattern.len())
            .saturating_add(256);
        if self.max_entries == 0 || charged > self.max_bytes {
            return Ok(value);
        }
        while self.charged_bytes > self.max_bytes - charged {
            self.evict_random();
        }
        let pattern: Rc<str> = pattern.into();
        self.entries
            .entry(key)
            .or_default()
            .insert(pattern.clone(), self.slots.len());
        self.slots.push(Entry {
            key,
            pattern: pattern.clone(),
            value: value.clone(),
            charged,
        });
        self.recent = Some((key, pattern, value.clone()));
        self.charged_bytes += charged;
        Ok(value)
    }

    fn evict_random(&mut self) {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        // Multiply-high gives a bounded, near-uniform index without a retry
        // loop or a linear scan through the hash table.
        let index = ((u128::from(self.random) * self.slots.len() as u128) >> 64) as usize;
        let removed = self.slots.swap_remove(index);
        if self
            .recent
            .as_ref()
            .is_some_and(|(_, pattern, _)| Rc::ptr_eq(pattern, &removed.pattern))
        {
            self.recent = None;
        }
        if let Some(patterns) = self.entries.get_mut(&removed.key) {
            patterns.remove(removed.pattern.as_ref());
            if patterns.is_empty() {
                self.entries.remove(&removed.key);
            }
        }
        if let Some(moved) = self.slots.get(index)
            && let Some(slot) = self
                .entries
                .get_mut(&moved.key)
                .and_then(|patterns| patterns.get_mut(moved.pattern.as_ref()))
        {
            *slot = index;
        }
        self.charged_bytes -= removed.charged;
        self.counters.evictions = self.counters.evictions.saturating_add(1);
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
    fn cyclic_workload_retains_more_than_seventy_percent_after_warmup() {
        let mut cache = MatcherCache::<u8, ()> {
            random: 0x81d1_7e9a_a415_296b,
            ..MatcherCache::default()
        };
        let patterns: Vec<_> = (0..300).map(|i| format!("pattern-{i}")).collect();
        let mut hits = 0;
        for round in 0..1100 {
            for pattern in &patterns {
                let mut compiled = false;
                cache
                    .get_or_compile(
                        pattern,
                        0,
                        || {
                            compiled = true;
                            Ok::<_, ()>(())
                        },
                        |_| 0,
                    )
                    .unwrap();
                if round >= 100 && !compiled {
                    hits += 1;
                }
            }
        }
        let ratio = hits as f64 / 300_000.0;
        eprintln!("300-pattern cycle with 256 slots: {ratio:.4}");
        assert!(ratio > 0.70, "cycle hit rate was {ratio:.4}");
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
    fn byte_budget_evicts_individually_and_preserves_caller_aliases() {
        let mut c = MatcherCache::<u8, ()>::default();
        let mut values = Vec::new();
        for pattern in ["first", "second", "third"] {
            values.push(
                c.get_or_compile(pattern, 0, || Ok::<_, ()>(()), |_| 12 << 20)
                    .unwrap(),
            );
        }
        assert_eq!(c.stats().entries, 2);
        assert_eq!(c.stats().evictions, 1);
        assert!(c.stats().charged_bytes <= CACHE_BYTES);
        c.set_limits(0, 0);
        assert!(values.iter().all(|value| Rc::strong_count(value) == 1));
        assert!(c.recent.is_none());
    }

    #[test]
    fn oversized_values_execute_without_cache_retention() {
        let mut c = MatcherCache::<u8, ()>::default();
        for size in [CACHE_BYTES, usize::MAX] {
            let value = c
                .get_or_compile("oversized", 0, || Ok::<_, ()>(()), |_| size)
                .unwrap();
            assert_eq!(Rc::strong_count(&value), 1);
            assert_eq!(c.stats().entries, 0);
            assert_eq!(c.charged_bytes, 0);
            assert!(c.recent.is_none());
        }
    }

    #[test]
    fn full_cache_releases_one_entry_before_compilation() {
        let mut c = MatcherCache::default();
        let mut aliases = Vec::new();
        for i in 0..CACHE_CAP {
            aliases.push(Rc::downgrade(&get(&mut c, &i.to_string(), (0, 0))));
        }
        c.get_or_compile(
            "overflow",
            (0, 0),
            || {
                assert_eq!(
                    aliases
                        .iter()
                        .filter(|weak| weak.upgrade().is_none())
                        .count(),
                    1
                );
                Ok::<_, ()>(String::new())
            },
            |v| v.capacity(),
        )
        .unwrap();
        assert_eq!(c.stats().entries, CACHE_CAP);
    }

    #[test]
    fn eviction_and_resize_preserve_pattern_key_identity() {
        let mut c = MatcherCache::default();
        c.set_limits(5, 2048);
        for i in 0..2000 {
            let pattern = format!("pattern-{}", i % 13);
            let key = ((i % 3) as u8, (i % 7) as u8);
            assert_eq!(*get(&mut c, &pattern, key), format!("{pattern}:{key:?}"));
            assert!(c.stats().entries <= 5);
            assert!(c.stats().charged_bytes <= 2048);
            if i % 100 == 0 {
                c.set_limits(1, 1024);
                c.set_limits(5, 2048);
            }
        }
    }

    #[test]
    fn recent_alias_is_released_even_when_compilation_fails() {
        let mut c = MatcherCache::default();
        c.set_limits(1, 4096);
        let weak = Rc::downgrade(&get(&mut c, "first", (0, 0)));
        assert!(
            c.get_or_compile(
                "invalid",
                (0, 0),
                || {
                    assert!(weak.upgrade().is_none());
                    Err::<String, _>("bad")
                },
                |v| v.capacity()
            )
            .is_err()
        );
        assert!(c.recent.is_none());
        assert_eq!(c.stats().entries, 0);
        assert_eq!(c.stats().charged_bytes, 0);
        for (entries, bytes) in [(0, 4096), (8, 0)] {
            c.set_limits(entries, bytes);
            let value = get(&mut c, "uncached", (0, 0));
            assert_eq!(Rc::strong_count(&value), 1);
            assert_eq!(c.stats().entries, 0);
        }
    }

    #[test]
    fn resizing_and_compile_errors_keep_accounting_and_recent_correct() {
        let mut c = MatcherCache::default();
        c.set_limits(2, 1024);
        get(&mut c, "first", (0, 0));
        get(&mut c, "second", (1, 0));
        let before = c.stats();
        assert!(
            c.get_or_compile(
                "invalid",
                (0, 0),
                || Err::<String, _>("bad"),
                |v| v.capacity()
            )
            .is_err()
        );
        assert_eq!(c.stats().misses, before.misses + 1);
        assert_eq!(c.stats().evictions, 1);
        assert_eq!(c.stats().entries, 1);
        get(&mut c, "last", (0, 0));
        c.set_limits(1, 1024);
        assert_eq!(c.stats().entries, 1);
        c.set_limits(8, 1);
        assert_eq!(c.stats().entries, 0);
        assert_eq!(c.stats().charged_bytes, 0);
        assert!(c.recent.is_none());
        let value = get(&mut c, "uncached", (0, 0));
        assert_eq!(Rc::strong_count(&value), 1);
        c.set_limits(8, 4096);
        get(&mut c, "cached", (0, 0));
        get(&mut c, "cached", (0, 0));
        assert_eq!(c.stats().entries, 1);
        assert_eq!(c.stats().hits, 1);
        assert!(c.stats().charged_bytes <= 4096);
    }
}
