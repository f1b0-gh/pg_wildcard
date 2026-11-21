// SPDX-License-Identifier: Apache-2.0
//! Backend-local compiled-pattern cache; no PostgreSQL pointers or row values.

use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

pub(crate) const CACHE_CAP: usize = 256;

pub(crate) struct MatcherCache<K, V> {
    entries: HashMap<K, HashMap<Rc<str>, Rc<V>>>,
    recent: Option<(K, Rc<str>, Rc<V>)>,
    len: usize,
}

impl<K: Copy + Eq + Hash, V> Default for MatcherCache<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            recent: None,
            len: 0,
        }
    }
}

impl<K: Copy + Eq + Hash, V> MatcherCache<K, V> {
    pub(crate) fn get_or_compile<E>(
        &mut self,
        pattern: &str,
        key: K,
        compile: impl FnOnce() -> Result<V, E>,
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
        let value = Rc::new(compile()?);
        if self.len >= CACHE_CAP {
            self.entries.clear();
            self.recent = None;
            self.len = 0;
        }
        let pattern: Rc<str> = pattern.into();
        self.entries
            .entry(key)
            .or_default()
            .insert(pattern.clone(), value.clone());
        self.recent = Some((key, pattern, value.clone()));
        self.len += 1;
        Ok(value)
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
        c.get_or_compile(p, k, || Ok::<_, ()>(format!("{p}:{k:?}")))
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
                .get_or_compile("event=*", k, || -> Result<String, ()> {
                    panic!("a cached pattern must not compile again")
                })
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
    fn capacity_eviction_drops_cache_ownership_and_errors_do_not_replace_hits() {
        let mut c = MatcherCache::default();
        let first = get(&mut c, "first", (0, 0));
        let weak = Rc::downgrade(&first);
        drop(first);
        for i in 1..CACHE_CAP {
            get(&mut c, &i.to_string(), (0, 0));
        }
        get(&mut c, "first", (0, 0));
        assert!(
            c.get_or_compile("invalid", (0, 0), || Err::<String, _>("bad"))
                .is_err()
        );
        assert!(weak.upgrade().is_some());
        get(&mut c, "overflow", (0, 0));
        assert!(
            weak.upgrade().is_none(),
            "eviction must release the recent entry too"
        );
    }
}
