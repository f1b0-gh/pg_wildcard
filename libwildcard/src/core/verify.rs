// SPDX-License-Identifier: Apache-2.0
//! Exact matching with ClickHouse semantics and a linear-time engine.

use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use regex_automata::{
    Input,
    meta::{Cache, Regex},
    util::syntax,
};
use thiserror::Error;

use crate::core::pattern::{self, Piece};
use crate::core::query::GramQuery;
use crate::core::re2::{self, Re2Error};
use crate::core::regex_plan::{self, PlanError};
use crate::core::views::View;

/// Compiled-size limits per pattern.
pub const SIZE_LIMIT: usize = 8 << 20;
pub const DFA_SIZE_LIMIT: usize = 8 << 20;

/// Maximum engine-reported scratch bytes retained between searches. A search
/// can temporarily exceed this allowance under the engine's separate limits.
pub const RETAINED_SEARCH_BYTES: usize = 1 << 20;

#[derive(Debug, Error)]
pub enum MatchError {
    #[error(transparent)]
    TooLong(#[from] pattern::PatternTooLong),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Re2(#[from] Re2Error),
    #[error("invalid pattern: {0}")]
    Compile(String),
}

/// Pattern language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Elasticsearch-style `*` / `?` with `\` escape, whole value.
    Wildcard,
    /// SQL `LIKE` (`%`, `_`, `\` escape), whole value.
    Like,
    /// RE2 regex, unanchored search.
    Regex { case_insensitive: bool },
}

impl FromStr for Kind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "wildcard" => Ok(Kind::Wildcard),
            "like" => Ok(Kind::Like),
            "regex" => Ok(Kind::Regex {
                case_insensitive: false,
            }),
            "iregex" => Ok(Kind::Regex {
                case_insensitive: true,
            }),
            _ => Err(format!(
                "unknown kind '{s}' (wildcard, like, regex, iregex)"
            )),
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Wildcard => "wildcard",
            Kind::Like => "like",
            Kind::Regex {
                case_insensitive: false,
            } => "regex",
            Kind::Regex {
                case_insensitive: true,
            } => "iregex",
        })
    }
}

/// Caller-owned search scratch for [`Matcher::is_match_with`].
///
/// Keep one cache per worker to search a shared matcher without locking its
/// internal scratch. Reuse with another matcher is safe: its first search drops
/// the previous engine state and rebinds the cache. The identity token does not
/// retain the matcher or its compiled regex. Scratch is allocated lazily and
/// discarded after searches that exceed the matcher's retention allowance.
#[derive(Debug)]
pub struct MatchCache {
    owner: Arc<()>,
    scratch: Option<Cache>,
}

impl MatchCache {
    /// Engine-reported retained scratch bytes, excluding the inline wrapper,
    /// identity token, allocator overhead and transient search allocations.
    pub fn memory_usage(&self) -> usize {
        self.scratch.as_ref().map_or(0, Cache::memory_usage)
    }
}

/// A planned and compiled predicate: candidate filter + exact verifier.
#[derive(Debug)]
pub struct Matcher {
    pub kind: Kind,
    pub view: View,
    pub plan: GramQuery,
    re: Regex,
    scratch_limit: usize,
    identity: Arc<()>,
    // One explicit cache preserves Send + Sync without hidden per-thread pools.
    scratch: Mutex<Option<Cache>>,
}

fn build(re: &str, ci: bool) -> Result<Regex, MatchError> {
    Regex::builder()
        .configure(
            Regex::config()
                .utf8_empty(false)
                .nfa_size_limit(Some(SIZE_LIMIT))
                .hybrid_cache_capacity(DFA_SIZE_LIMIT),
        )
        .syntax(
            syntax::Config::new()
                .utf8(false)
                .unicode(true)
                .case_insensitive(ci),
        )
        .build(re)
        .map_err(|e| MatchError::Compile(e.to_string()))
}

/// Parse a whole-value pattern into pieces, case-folding literals for views
/// that fold case.
/// Low-level, unbounded helper; use [`pieces_for_checked`] for untrusted input.
pub fn pieces_for(kind: Kind, pattern: &[u8], view: View) -> Vec<Piece> {
    let pieces = match kind {
        Kind::Like => pattern::parse_like(pattern),
        _ => pattern::parse_wildcard(pattern),
    };
    if !view.folds_case() {
        return pieces;
    }
    pieces
        .into_iter()
        .map(|p| match p {
            Piece::Lit(v) => Piece::Lit(view.pattern_literal(&v)),
            other => other,
        })
        .collect()
}

pub fn pieces_for_checked(
    kind: Kind,
    pattern: &[u8],
    view: View,
) -> Result<Vec<Piece>, pattern::PatternTooLong> {
    pattern::check_pattern_len(pattern)?;
    Ok(pieces_for(kind, pattern, view))
}

impl Matcher {
    fn compiled(kind: Kind, view: View, plan: GramQuery, re: Regex) -> Self {
        // This chooses a retention allowance, not an estimate or guarantee of
        // future search memory. Small patterns can share the backend budget;
        // searches exceeding their allowance release their scratch on return.
        let scratch_limit = re
            .memory_usage()
            .saturating_mul(16)
            .clamp(64 << 10, RETAINED_SEARCH_BYTES);
        Self {
            kind,
            view,
            plan,
            re,
            scratch_limit,
            identity: Arc::new(()),
            scratch: Mutex::new(None),
        }
    }

    pub fn new(kind: Kind, pattern: &str, view: View) -> Result<Self, MatchError> {
        pattern::check_pattern_len(pattern.as_bytes())?;
        match kind {
            Kind::Wildcard | Kind::Like => {
                let pieces = pieces_for(kind, pattern.as_bytes(), view);
                Ok(Self::compiled(
                    kind,
                    view,
                    pattern::plan(&pieces, view),
                    build(&pattern::to_regex(&pieces), false)?,
                ))
            }
            Kind::Regex { case_insensitive } => {
                let rust = re2::to_rust(pattern)?;
                let plan = regex_plan::plan_translated_regex(&rust, view, case_insensitive)?;
                Ok(Self::compiled(
                    kind,
                    view,
                    plan,
                    build(&rust, case_insensitive)?,
                ))
            }
        }
    }

    pub fn wildcard(pattern: &str, view: View) -> Result<Self, MatchError> {
        Self::new(Kind::Wildcard, pattern, view)
    }

    pub fn like(pattern: &str, view: View) -> Result<Self, MatchError> {
        Self::new(Kind::Like, pattern, view)
    }

    pub fn regex(pattern: &str, view: View, case_insensitive: bool) -> Result<Self, MatchError> {
        Self::new(Kind::Regex { case_insensitive }, pattern, view)
    }

    /// Exact check against the raw value (the view transform is applied here).
    pub fn is_match(&self, raw: &[u8]) -> bool {
        let value = self.view.apply(raw);
        let mut scratch = self
            .scratch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.search_with_scratch(&mut scratch, value.as_ref())
    }

    /// Create lazily allocated caller-owned scratch, independent of the mutex
    /// used by [`Self::is_match`]. Each worker should keep its own cache.
    pub fn create_cache(&self) -> MatchCache {
        MatchCache {
            owner: Arc::clone(&self.identity),
            scratch: None,
        }
    }

    /// Exact check using caller-owned scratch, without locking shared state.
    /// Applies the view to the raw bytes exactly as [`Self::is_match`] does.
    /// A cache from any matcher is accepted and safely rebound when necessary.
    ///
    /// ```
    /// use libwildcard::core::{Matcher, View};
    /// let matcher = Matcher::wildcard("*evil*", View::Lower)?;
    /// let mut cache = matcher.create_cache();
    /// assert!(matcher.is_match_with(&mut cache, b"EVIL.example"));
    /// # Ok::<(), libwildcard::core::verify::MatchError>(())
    /// ```
    pub fn is_match_with(&self, cache: &mut MatchCache, raw: &[u8]) -> bool {
        if !Arc::ptr_eq(&cache.owner, &self.identity) {
            cache.scratch = None;
            cache.owner = Arc::clone(&self.identity);
        }
        let value = self.view.apply(raw);
        self.search_with_scratch(&mut cache.scratch, value.as_ref())
    }

    fn search_with_scratch(&self, scratch: &mut Option<Cache>, value: &[u8]) -> bool {
        let cache = scratch.get_or_insert_with(|| self.re.create_cache());
        let matched = self
            .re
            .search_half_with(cache, &Input::new(value).earliest(true))
            .is_some();
        if cache.memory_usage() > self.scratch_limit {
            *scratch = None;
        }
        matched
    }

    /// Charge for cache admission: engine-reported approximate compiled heap,
    /// plan vector capacities, inline matcher and reserved retained scratch.
    /// This is not allocator accounting or a process RSS bound. Compile/search
    /// temporaries, caller-owned values and allocator overhead are excluded.
    /// The allowance covers only the internal mutex's scratch. Every caller-owned
    /// [`MatchCache`] has its own additional allowance (at most
    /// [`RETAINED_SEARCH_BYTES`]); callers must bound their number of caches.
    pub fn cache_charge(&self) -> usize {
        fn plan_heap(plan: &GramQuery) -> usize {
            match plan {
                GramQuery::And(parts) | GramQuery::Or(parts) => parts.iter().fold(
                    parts
                        .capacity()
                        .saturating_mul(std::mem::size_of::<GramQuery>()),
                    |bytes, child| bytes.saturating_add(plan_heap(child)),
                ),
                _ => 0,
            }
        }
        std::mem::size_of::<Self>()
            // Arc identity's two reference counters; allocation overhead excluded.
            .saturating_add(2 * std::mem::size_of::<usize>())
            .saturating_add(self.re.memory_usage())
            .saturating_add(plan_heap(&self.plan))
            .saturating_add(self.scratch_limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grams::value_grams;

    #[test]
    fn caller_owned_cache_rebinds_across_matchers_and_views() {
        let matchers = [
            Matcher::regex("a{100}b", View::Raw, false).unwrap(),
            Matcher::wildcard("*evil*", View::Lower).unwrap(),
            Matcher::regex("(?i)k", View::Raw, false).unwrap(),
            Matcher::like("a_b", View::Raw).unwrap(),
        ];
        let long = format!("{}b", "a".repeat(100));
        let values: &[&[u8]] = &[
            b"",
            b"\xff",
            b"\0",
            b"EVIL",
            long.as_bytes(),
            "aéb".as_bytes(),
            "K".as_bytes(),
        ];
        let mut cache = matchers[0].create_cache();
        for _ in 0..3 {
            for matcher in &matchers {
                for value in values {
                    assert_eq!(
                        matcher.is_match_with(&mut cache, value),
                        matcher.is_match(value)
                    );
                }
            }
        }
        // The cache may outlive the matcher that created it. A new matcher must
        // never interpret scratch left by a previous regex as its own state.
        let mut cache = {
            let matcher = Matcher::regex("a+", View::Raw, false).unwrap();
            let mut cache = matcher.create_cache();
            assert!(matcher.is_match_with(&mut cache, b"aaa"));
            cache
        };
        let matcher = Matcher::regex("b+", View::Raw, false).unwrap();
        assert!(!matcher.is_match_with(&mut cache, b"aaa"));
        assert!(matcher.is_match_with(&mut cache, b"bbb"));
    }

    #[test]
    fn caller_owned_cache_discards_scratch_over_retention_limit() {
        let mut matcher = Matcher::regex("a{1000}b", View::Raw, false).unwrap();
        matcher.scratch_limit = 0;
        let mut cache = matcher.create_cache();
        let value = format!("{}b", "a".repeat(1000));
        assert!(matcher.is_match_with(&mut cache, value.as_bytes()));
        assert!(cache.scratch.is_none());
        assert_eq!(cache.memory_usage(), 0);
        // Discarding scratch must not change subsequent results.
        assert!(!matcher.is_match_with(&mut cache, b"aaa"));
        assert!(cache.scratch.is_none());
    }

    #[test]
    fn caller_owned_cache_has_bounded_retention_without_shared_scratch() {
        let matcher = Matcher::regex("a{100}b", View::Raw, false).unwrap();
        let value = format!("{}b", "a".repeat(100));
        let charge = matcher.cache_charge();
        let mut cache = matcher.create_cache();
        assert!(matcher.is_match_with(&mut cache, value.as_bytes()));
        assert!(cache.memory_usage() <= matcher.scratch_limit);
        assert!(matcher.scratch.lock().unwrap().is_none());
        assert_eq!(matcher.cache_charge(), charge);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let matcher = &matcher;
                let value = &value;
                scope.spawn(move || {
                    let mut cache = matcher.create_cache();
                    for _ in 0..10 {
                        assert!(matcher.is_match_with(&mut cache, value.as_bytes()));
                    }
                });
            }
        });
        assert!(matcher.scratch.lock().unwrap().is_none());
    }

    #[test]
    fn oversized_patterns_are_rejected_before_parsing() {
        let patterns = ["%".repeat(16_385), "a".repeat(16_385)];
        for pattern in patterns {
            for kind in [
                Kind::Like,
                Kind::Wildcard,
                Kind::Regex {
                    case_insensitive: false,
                },
            ] {
                let error = Matcher::new(kind, &pattern, View::Raw).unwrap_err();
                assert!(error.to_string().contains("pattern exceeds"), "{error}");
            }
            assert!(
                re2::to_rust(&pattern)
                    .unwrap_err()
                    .to_string()
                    .contains("pattern exceeds")
            );
            assert!(
                regex_plan::plan_regex(&pattern, View::Raw, false)
                    .unwrap_err()
                    .to_string()
                    .contains("pattern exceeds")
            );
        }
    }

    #[test]
    fn admitted_source_may_expand_during_re2_translation() {
        let source = r"\d".repeat(pattern::MAX_PATTERN_BYTES / 2);
        assert!(re2::to_rust(&source).unwrap().len() > pattern::MAX_PATTERN_BYTES);
        assert!(Matcher::regex(&source, View::Raw, false).is_ok());
    }

    #[test]
    fn small_patterns_leave_room_for_256_cache_entries() {
        for kind in [
            Kind::Like,
            Kind::Wildcard,
            Kind::Regex {
                case_insensitive: false,
            },
        ] {
            let matcher = Matcher::new(kind, "event=login", View::Raw).unwrap();
            assert!((matcher.cache_charge() + 256 + "event=login".len()) * 256 <= 32 << 20);
        }
    }

    #[test]
    fn explicit_scratch_preserves_send_sync_and_retention_policy() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Matcher>();
        let pattern = format!("{}b", "a{1000}".repeat(8));
        let matcher = Matcher::regex(&pattern, View::Raw, false).unwrap();
        let value = format!("{}b", "a".repeat(8000));
        let charge = matcher.cache_charge();
        assert!(matcher.is_match(value.as_bytes()));
        assert!(
            matcher
                .scratch
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|cache| cache.memory_usage() <= matcher.scratch_limit),
            "retained scratch must stay within its reservation"
        );
        assert_eq!(matcher.cache_charge(), charge);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| assert!(matcher.is_match(value.as_bytes())));
            }
        });
    }

    #[test]
    fn explicit_engine_matches_regex_bytes_configuration() {
        let patterns = [
            "", "a.b", "(?i)k", r"\babc\b", "^", "$", "a{1000}b", "(a|ab)*b", "(?-s:.)",
        ];
        let values: &[&[u8]] = &[
            b"",
            b"\xff",
            b"a\xffb",
            b"a\nb",
            b"abc",
            b"\0abc\0",
            "K".as_bytes(),
            b"aab",
            b"\n",
        ];
        for pattern in patterns {
            for ci in [false, true] {
                let rust = re2::to_rust(pattern).unwrap();
                let reference = regex::bytes::RegexBuilder::new(&rust)
                    .unicode(true)
                    .case_insensitive(ci)
                    .size_limit(SIZE_LIMIT)
                    .dfa_size_limit(DFA_SIZE_LIMIT)
                    .build()
                    .unwrap();
                let matcher = Matcher::regex(pattern, View::Raw, ci).unwrap();
                for value in values {
                    assert_eq!(
                        matcher.is_match(value),
                        reference.is_match(value),
                        "{pattern:?}, {ci}, {value:?}"
                    );
                }
            }
        }
    }

    /// Deterministic xorshift for property tests (no external crate).
    pub(crate) struct Rng(pub u64);
    impl Rng {
        pub fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        pub fn pick<T: Copy>(&mut self, v: &[T]) -> T {
            v[(self.next() % v.len() as u64) as usize]
        }
    }

    /// Tokens include a 2-byte character, newline, NUL and an invalid byte.
    const TOKENS: &[&[u8]] = &[
        b"a",
        b"b",
        b"c",
        b"A",
        b"C",
        b".",
        b"/",
        b"\\",
        b"C:",
        b"%",
        b"2e",
        b"E",
        b"-",
        b"_",
        b" ",
        b"\n",
        b"\x00",
        b"\xff",
        "é".as_bytes(),
    ];

    fn random_value(r: &mut Rng) -> Vec<u8> {
        let n = (r.next() % 12) as usize;
        (0..n).flat_map(|_| r.pick(TOKENS).to_vec()).collect()
    }

    fn assert_sound(m: &Matcher, values: &[Vec<u8>], what: &str) {
        for v in values {
            if m.is_match(v) {
                let g = value_grams(m.view, &m.view.apply(v));
                assert!(
                    m.plan.eval_sorted(&g),
                    "false negative: {what} on {:?} plan {}",
                    String::from_utf8_lossy(v),
                    m.plan
                );
            }
        }
    }

    #[test]
    fn like_and_wildcard_never_miss() {
        let mut r = Rng(0x5eed_1234_abcd_0001);
        let values: Vec<Vec<u8>> = (0..400).map(|_| random_value(&mut r)).collect();
        let mut matched = 0;
        for _ in 0..500 {
            let n = 1 + (r.next() % 6) as usize;
            let pat: String = (0..n)
                .map(|_| {
                    r.pick(&[
                        "a", "b", "c", "A", ".", "%", "_", "*", "?", "e", "2", "é", "/", r"\\",
                        "C:",
                    ])
                })
                .collect();
            for view in View::ALL {
                for kind in [Kind::Like, Kind::Wildcard] {
                    let m = Matcher::new(kind, &pat, view).unwrap();
                    matched += values.iter().filter(|v| m.is_match(v)).count();
                    assert_sound(&m, &values, &format!("{kind} {pat:?} {view}"));
                }
            }
        }
        assert!(
            matched > 1000,
            "test corpus should produce matches, got {matched}"
        );
    }

    #[test]
    fn regex_never_misses() {
        let mut r = Rng(0x0dd_ba11_cafe_f00d);
        let values: Vec<Vec<u8>> = (0..400).map(|_| random_value(&mut r)).collect();
        let atoms = [
            "a",
            "b",
            "ab",
            "abc",
            "A",
            r"\.",
            "%2e",
            "[ab]",
            ".",
            "(a|bc)",
            "(abc|b.)",
            "c?",
            "a+",
            "^",
            "$",
            "E",
            r"\w",
            r"\d",
            r"\s",
            r"\b",
            "é",
            "a[bc]d",
            "ab(cd|ef)gh",
            "(ab|)",
            "[ab]{2,4}",
            "[éK]a",
            "(?i:cmd)",
        ];
        let mut matched = 0;
        for _ in 0..600 {
            let n = 1 + (r.next() % 4) as usize;
            let re: String = (0..n).map(|_| r.pick(&atoms)).collect();
            for view in View::ALL {
                for ci in [false, true] {
                    let m = Matcher::regex(&re, view, ci).unwrap();
                    matched += values.iter().filter(|v| m.is_match(v)).count();
                    assert_sound(&m, &values, &format!("regex {re:?} {view} ci={ci}"));
                }
            }
        }
        assert!(matched > 1000, "got {matched}");
    }

    #[test]
    fn finite_regex_expansion_never_misses() {
        let mut r = Rng(0x2026_1115_00ab_cdef);
        // Pair each fragment with a witness, so generated branches actually
        // match rather than passing the implication only on random negatives.
        let fragments = [
            ("a[bc]d", "abd"),
            ("a[bc]d", "acd"),
            ("ab(cd|ef)gh", "abcdgh"),
            ("ab(cd|ef)gh", "abefgh"),
            ("(ab|)", ""),
            ("(ab|)", "ab"),
            ("[ab]{2,4}", "abba"),
            ("[ab]{2,4}", "aa"),
            ("[a-z]{6}", "zzzzzz"),
            ("a?bc", "bc"),
            ("[éK]x", "éx"),
            ("[éK]x", "Kx"),
            ("(?i:k)xy", "Kxy"),
            ("(?i:cmd)", "CMD"),
            (
                "x{80}",
                "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            ),
            (r"\x00", "\0"),
        ];
        let mut matched = 0;
        for _ in 0..160 {
            let parts: Vec<_> = (0..1 + r.next() % 5).map(|_| r.pick(&fragments)).collect();
            let pattern: String = parts.iter().map(|p| p.0).collect();
            let value: String = parts.iter().map(|p| p.1).collect();
            for pattern in [
                pattern.clone(),
                format!("^{pattern}$"),
                format!(".*({pattern}).*"),
            ] {
                for view in View::ALL {
                    for ci in [false, true] {
                        let m = Matcher::regex(&pattern, view, ci).unwrap();
                        let values = vec![
                            value.as_bytes().to_vec(),
                            format!("!{value}!").into_bytes(),
                            [b"\xff".as_slice(), value.as_bytes(), b"\0"].concat(),
                        ];
                        matched += values.iter().filter(|v| m.is_match(v)).count();
                        assert_sound(&m, &values, &pattern);
                    }
                }
            }
        }
        assert!(matched > 1000, "vacuous corpus: {matched}");
    }

    #[test]
    fn clickhouse_semantics() {
        // `_` is one character, not one byte (é is 2 bytes)
        assert!(
            Matcher::like("a_b", View::Raw)
                .unwrap()
                .is_match("aéb".as_bytes())
        );
        assert!(
            !Matcher::like("a__b", View::Raw)
                .unwrap()
                .is_match("aéb".as_bytes())
        );
        // wildcards never match invalid UTF-8
        assert!(!Matcher::like("a%b", View::Raw).unwrap().is_match(b"a\xffb"));
        // `%` and `.` match newline
        assert!(Matcher::like("a%b", View::Raw).unwrap().is_match(b"a\nb"));
        assert!(
            Matcher::regex("a.b", View::Raw, false)
                .unwrap()
                .is_match(b"a\nb")
        );
        // `$` is end of text only
        assert!(
            !Matcher::regex("b$", View::Raw, false)
                .unwrap()
                .is_match(b"ab\n")
        );
        // ASCII-only classes
        assert!(
            !Matcher::regex(r"\d", View::Raw, false)
                .unwrap()
                .is_match("٣".as_bytes())
        );
        assert!(
            !Matcher::regex(r"\w", View::Raw, false)
                .unwrap()
                .is_match("é".as_bytes())
        );
        // Unicode case folding with (?i), like RE2
        assert!(
            Matcher::regex("(?i)k", View::Raw, false)
                .unwrap()
                .is_match("\u{212a}".as_bytes())
        );
    }

    #[test]
    fn security_examples() {
        let m = Matcher::wildcard("*/wp-admin/*.php", View::Decoded).unwrap();
        assert!(m.is_match(b"GET /blog/%77p-ADMIN/setup-config.PHP"));
        assert!(!m.plan.is_all());

        let m = Matcher::like("com.evil.%", View::RevDomain).unwrap();
        assert!(m.is_match(b"https://login.EVIL.com/x"));
        assert!(!m.is_match(b"https://evil.com.attacker.net/"));

        let m = Matcher::regex(
            r"powershell(\.exe)?\s+-e(nc|ncodedcommand)?\s",
            View::Lower,
            true,
        )
        .unwrap();
        assert!(m.is_match(b"C:\\Windows\\PowerShell.EXE -EncodedCommand SQBFAFgA"));

        let m = Matcher::regex("(a+)+$", View::Raw, false).unwrap();
        let mut v = vec![b'a'; 50_000];
        v.push(b'!');
        let t = std::time::Instant::now();
        assert!(!m.is_match(&v));
        assert!(t.elapsed().as_millis() < 500, "took {:?}", t.elapsed());
    }

    #[test]
    fn invalid_patterns_error() {
        assert!(Matcher::regex("(", View::Raw, false).is_err());
        assert!(Matcher::regex("(a)\\1", View::Raw, false).is_err());
        assert!(Matcher::regex("[a&&b]", View::Raw, false).is_err());
    }
}
