// SPDX-License-Identifier: Apache-2.0
//! Per-backend cache configuration and counters; never called by FDW workers.

use libwildcard::core::{Matcher, verify::Kind, views::View};
use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};
use pgrx::prelude::*;

use crate::matcher_cache::{CACHE_BYTES, CACHE_CAP, MatcherCache};

static ENTRIES: GucSetting<i32> = GucSetting::<i32>::new(CACHE_CAP as i32);
static KILOBYTES: GucSetting<i32> = GucSetting::<i32>::new((CACHE_BYTES / 1024) as i32);

pub(super) fn init() {
    GucRegistry::define_int_guc(
        c"pg_wildcard.matcher_cache_entries",
        c"Maximum retained compiled patterns per backend.",
        c"Zero disables retention. Changes apply on the next match or diagnostics call.",
        &ENTRIES,
        0,
        65_536,
        GucContext::Userset,
        GucFlags::default(),
    );
    GucRegistry::define_int_guc(
        c"pg_wildcard.matcher_cache_kb",
        c"Maximum accounted retained matcher-cache memory per backend, in KiB.",
        c"Not an RSS bound: compilation/search temporaries and caller-owned aliases are excluded. Zero disables retention.",
        &KILOBYTES,
        0,
        1_048_576,
        GucContext::Userset,
        GucFlags::UNIT_KB,
    );
}

pub(super) fn configure(cache: &mut MatcherCache<(Kind, View), Matcher>) {
    cache.set_limits(ENTRIES.get() as usize, (KILOBYTES.get() as usize) * 1024);
}

/// Backend-local counters, including evictions caused by lowering either limit.
#[pg_extern]
fn wc_matcher_cache_stats() -> TableIterator<
    'static,
    (
        name!(entries, i64),
        name!(charged_bytes, i64),
        name!(hits, i64),
        name!(misses, i64),
        name!(evictions, i64),
    ),
> {
    let stats = crate::CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        configure(&mut cache);
        cache.stats()
    });
    let count = |n: u64| n.min(i64::MAX as u64) as i64;
    TableIterator::once((
        stats.entries as i64,
        stats.charged_bytes as i64,
        count(stats.hits),
        count(stats.misses),
        count(stats.evictions),
    ))
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use super::*;

    #[pg_test]
    fn matcher_configuration_and_counters_follow_session_limits() {
        Spi::run("SET LOCAL pg_wildcard.matcher_cache_entries = 2; SET LOCAL pg_wildcard.matcher_cache_kb = '1MB'").unwrap();
        let before = wc_matcher_cache_stats().next().unwrap();
        for _ in 0..2 {
            assert!(
                crate::matcher("cache-config-unique*", Kind::Wildcard, View::Raw)
                    .is_match(b"cache-config-unique-value")
            );
        }
        let after = wc_matcher_cache_stats().next().unwrap();
        assert!(after.2 > before.2, "repeat match should count as a hit");
        assert!(after.3 > before.3, "first match should count as a miss");
        assert!(after.0 <= 2 && after.1 <= 1024 * 1024);
        assert_eq!(
            Spi::get_one::<i64>("SELECT entries FROM wc_matcher_cache_stats()").unwrap(),
            Some(after.0)
        );

        Spi::run("SET LOCAL pg_wildcard.matcher_cache_kb = 0").unwrap();
        let disabled = wc_matcher_cache_stats().next().unwrap();
        assert_eq!((disabled.0, disabled.1), (0, 0));
        assert!(disabled.4 > after.4);
        for _ in 0..2 {
            assert!(
                crate::matcher("cache-config-unique*", Kind::Wildcard, View::Raw)
                    .is_match(b"cache-config-unique-value")
            );
        }
        let empty = wc_matcher_cache_stats().next().unwrap();
        assert_eq!((empty.0, empty.1), (0, 0));
        assert_eq!(empty.3 - disabled.3, 2);

        Spi::run("SET LOCAL pg_wildcard.matcher_cache_kb = '1MB'; SET LOCAL pg_wildcard.matcher_cache_entries = 0").unwrap();
        crate::matcher("cache-config-other*", Kind::Wildcard, View::Raw);
        assert_eq!(wc_matcher_cache_stats().next().unwrap().0, 0);
    }
}
