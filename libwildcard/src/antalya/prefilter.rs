// SPDX-License-Identifier: Apache-2.0
//! Conservative native candidate filters independent of tokenizer metadata.
use super::{ChError, like_pattern, predicate, quote};
use crate::core::{
    GramQuery, View, grams,
    pattern::Piece,
    verify::{Kind, Matcher},
};

/// Optional candidate filter on an already transformed value expression.
///
/// Uses scalar LIKE semantics, so an absent or differently configured text index
/// cannot introduce false negatives. The engine may accelerate these predicates
/// with its text-index hints. Callers must retain the exact predicate and should
/// opt in only after measuring their distribution. SQL expressions are trusted
/// codegen inputs, as for `predicate`; analyst text is always quoted.
/// Unsupported grams widen, and an unsupported OR branch disables that branch's
/// whole disjunction. Bounded independently of the existing gram planner.
pub fn candidate_predicate(
    value_expr: &str,
    kind: Kind,
    pattern: &str,
    view: View,
) -> Result<Option<String>, ChError> {
    // Preserve validation/semantics of the exact API before optional planning.
    let _ = predicate(value_expr, kind, pattern, view)?;
    if pattern.len() > 8192 {
        return Ok(None);
    }
    let Ok(matcher) = Matcher::new(kind, pattern, view) else {
        return Ok(None);
    };
    let mut budget = 64;
    Ok(emit(value_expr, &matcher.plan, 0, &mut budget))
}

/// Observed distinct-file counts keyed by the complete, view-aware gram key.
/// Missing keys are unknown, never zero. Counts influence ranking only.
pub type GramFileCounts = std::collections::BTreeMap<grams::GramKey, u64>;

/// Necessary byte substrings for a dynamic RE2 watchlist, using fixed ranking.
/// See [`watchlist_needles_with_frequencies`] for corpus-aware selection.
pub fn watchlist_needles(pattern: &str, view: View) -> Result<Vec<String>, ChError> {
    watchlist_needles_with_frequencies(pattern, view, None)
}

/// Select at most three required printable ASCII interior trigrams from a
/// bounded pure conjunction. Unsupported plans widen to exact matching only.
/// Known file counts rank ascending; unknown counts rank after known counts.
/// A fixed heuristic breaks ties and ranks missing statistics: punctuation and
/// common URL grams are penalized, digits and rare ASCII letters preferred.
/// Counts (even stale or incorrect ones) never establish that a gram is required.
/// Validation and regex/view binding remain the exact API's responsibility.
pub fn watchlist_needles_with_frequencies(
    pattern: &str,
    view: View,
    frequencies: Option<&GramFileCounts>,
) -> Result<Vec<String>, ChError> {
    let kind = Kind::Regex {
        case_insensitive: false,
    };
    let _ = predicate("v", kind, pattern, view)?;
    Ok(select_needles(
        required_grams(pattern, view).unwrap_or_default(),
        frequencies,
    ))
}

// Every returned gram is required. Reject the whole optional plan on unsupported
// nodes, anchors, nonprintable bytes or exhausted bounds; never infer an OR gram.
fn required_grams(pattern: &str, view: View) -> Option<Vec<grams::GramKey>> {
    if pattern.len() > 8192 {
        return None;
    }
    let matcher = Matcher::regex(pattern, view, false).ok()?;
    fn collect(
        query: &GramQuery,
        view: View,
        depth: usize,
        budget: &mut usize,
        out: &mut Vec<grams::GramKey>,
    ) -> bool {
        if depth >= 32 || *budget == 0 {
            return false;
        }
        *budget -= 1;
        match query {
            GramQuery::Gram(g) => {
                let (binding, symbols) = grams::unpack(*g);
                if binding != Some(view) || !symbols.iter().all(|s| (32..127).contains(s)) {
                    return false;
                }
                out.push(*g);
                true
            }
            GramQuery::And(children) => children
                .iter()
                .all(|q| collect(q, view, depth + 1, budget, out)),
            GramQuery::All | GramQuery::Or(_) => false,
        }
    }
    let mut grams = Vec::new();
    if !collect(&matcher.plan, view, 0, &mut 64, &mut grams) {
        return None;
    }
    grams.sort_unstable();
    grams.dedup();
    Some(grams)
}

fn select_needles(
    mut needles: Vec<grams::GramKey>,
    frequencies: Option<&GramFileCounts>,
) -> Vec<String> {
    needles.sort_by_key(|g| {
        let count = frequencies.and_then(|counts| counts.get(g)).copied();
        (
            count.is_none(),
            count.unwrap_or(u64::MAX),
            fallback_rank(*g),
            *g,
        )
    });
    needles.truncate(3);
    needles
        .into_iter()
        .map(|g| {
            grams::unpack(g)
                .1
                .iter()
                .map(|s| char::from(*s as u8))
                .collect()
        })
        .collect()
}

/// Optional row gate: the intersection contains only grams required by every
/// indicator. Mixed views or any unsupported plan disable the entire gate.
pub(super) fn common_required_needles(items: &[super::SweepItem]) -> Option<(View, Vec<String>)> {
    let view = items.first()?.view;
    let mut common: Option<Vec<grams::GramKey>> = None;
    for item in items {
        if item.view != view {
            return None;
        }
        let grams = required_grams(&item.regex, view)?;
        match &mut common {
            None => common = Some(grams),
            Some(shared) => shared.retain(|g| grams.binary_search(g).is_ok()),
        }
        if common.as_ref()?.is_empty() {
            return None;
        }
    }
    Some((view, select_needles(common?, None)))
}

fn fallback_rank(gram: grams::GramKey) -> u16 {
    let bytes = grams::unpack(gram).1.map(|s| s as u8);
    let lower = bytes.map(|b| b.to_ascii_lowercase());
    let common = matches!(
        &lower,
        b"htt"
            | b"ttp"
            | b"tps"
            | b"www"
            | b"com"
            | b"net"
            | b"org"
            | b"exa"
            | b"xam"
            | b"amp"
            | b"mpl"
            | b"ple"
    );
    let score: u16 = bytes
        .iter()
        .map(|b| match b.to_ascii_lowercase() {
            b'0'..=b'9' => 0,
            b'j' | b'q' | b'x' | b'z' => 1,
            b'k' | b'v' | b'b' | b'p' | b'y' => 3,
            b'a'..=b'z' => 6,
            _ => 32,
        })
        .sum();
    score + if common { 96 } else { 0 }
}

fn emit(column: &str, query: &GramQuery, depth: usize, budget: &mut usize) -> Option<String> {
    if depth >= 32 || *budget == 0 {
        return None;
    }
    *budget -= 1;
    match query {
        GramQuery::All => None,
        GramQuery::Gram(g) => {
            let (_, symbols) = grams::unpack(*g);
            // Interior printable ASCII only: no anchors, UTF-8 fragments or NUL.
            if !symbols.iter().all(|s| (32..127).contains(s)) {
                return None;
            }
            let bytes = symbols.iter().map(|s| *s as u8).collect();
            let pat = like_pattern(&[Piece::AnyMany, Piece::Lit(bytes), Piece::AnyMany]);
            Some(format!("like({column}, {})", quote(&pat)))
        }
        GramQuery::And(children) => {
            let parts: Vec<_> = children
                .iter()
                .filter_map(|q| emit(column, q, depth + 1, budget))
                .collect();
            (!parts.is_empty()).then(|| format!("({})", parts.join(" AND ")))
        }
        GramQuery::Or(children) => {
            let parts: Option<Vec<_>> = children
                .iter()
                .map(|q| emit(column, q, depth + 1, budget))
                .collect();
            let parts = parts?;
            (!parts.is_empty()).then(|| format!("({})", parts.join(" OR ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn common_items(patterns: &[&str], view: View) -> Vec<crate::antalya::SweepItem> {
        patterns
            .iter()
            .enumerate()
            .map(|(id, p)| crate::antalya::SweepItem {
                id: id as i64,
                view,
                regex: (*p).into(),
            })
            .collect()
    }
    #[test]
    fn shared_grams_are_required_for_every_matching_item_and_view() {
        let values = [
            b"abc123def".as_slice(),
            b"abc456ghi",
            b"\xff\0abc123def",
            b"ABC123DEF",
            b"https://abc123def/x",
            b"%61bc123def",
            b"\xc3\xa9abc456ghi",
            b"",
            b"abc123def\nabc456ghi",
        ];
        for view in View::ALL {
            let list = common_items(&["abc.*def", "abc.*ghi"], view);
            let (_, needles) = common_required_needles(&list).unwrap();
            assert_eq!(needles, vec!["abc"]);
            for item in &list {
                let matcher = Matcher::regex(&item.regex, view, false).unwrap();
                for value in values {
                    if matcher.is_match(value) {
                        let transformed = view.apply(value);
                        for needle in &needles {
                            assert!(
                                transformed
                                    .windows(needle.len())
                                    .any(|w| w == needle.as_bytes())
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn unknown_or_mixed_plans_disable_the_gate() {
        for patterns in [
            vec!["abc", "def"],
            vec!["abc", "abc|def"],
            vec!["abc", "(?i)abc"],
            vec!["abc", "^abc$"],
        ] {
            assert!(common_required_needles(&common_items(&patterns, View::Raw)).is_none());
        }
        let mut mixed = common_items(&["abc", "abc"], View::Raw);
        mixed[1].view = View::Lower;
        assert!(common_required_needles(&mixed).is_none());
        assert!(common_required_needles(&[]).is_none());
        assert!(common_required_needles(&common_items(&[&"x".repeat(8193)], View::Raw)).is_none());
    }
    #[test]
    fn watchlist_needles_require_bounded_safe_conjunctions() {
        assert_eq!(watchlist_needles("abc", View::Raw).unwrap(), vec!["abc"]);
        assert_eq!(
            watchlist_needles("abcdefgh", View::Raw).unwrap(),
            vec!["abc", "bcd", "cde"]
        );
        for pattern in ["", "a", "é", "abc|xyz", "(?i)abc", "^abc$", "\0", "\u{ff}"] {
            assert!(
                watchlist_needles(pattern, View::Raw).unwrap().is_empty(),
                "{pattern:?}"
            );
        }
        assert!(
            watchlist_needles(&"x".repeat(8193), View::Raw)
                .unwrap()
                .is_empty()
        );
        assert!(watchlist_needles("(?=x)", View::Raw).is_err());
    }

    #[test]
    fn frequency_ranking_is_view_aware_and_missing_is_unknown() {
        let key = |view, b: &[u8; 3]| grams::key(view, b[0].into(), b[1].into(), b[2].into());
        let counts = GramFileCounts::from([
            (key(View::Raw, b"abc"), 99),
            (key(View::Raw, b"def"), 2),
            (key(View::Raw, b"ghi"), 0),
            (key(View::Lower, b"jkl"), 0),
        ]);
        assert_eq!(
            watchlist_needles_with_frequencies("abc.*def.*ghi.*jkl", View::Raw, Some(&counts))
                .unwrap(),
            vec!["ghi", "def", "abc"]
        );
        assert_eq!(
            watchlist_needles_with_frequencies("abc.*def.*ghi.*jkl", View::Lower, Some(&counts))
                .unwrap()[0],
            "jkl"
        );
        assert_eq!(
            watchlist_needles_with_frequencies("abc", View::Raw, Some(&counts)).unwrap(),
            vec!["abc"]
        );
        assert!(
            watchlist_needles_with_frequencies("abc|def", View::Raw, Some(&counts))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            watchlist_needles_with_frequencies("abcdef", View::Raw, Some(&GramFileCounts::new()))
                .unwrap(),
            watchlist_needles("abcdef", View::Raw).unwrap()
        );
    }

    #[test]
    fn fixed_ranking_penalizes_common_url_and_punctuation_grams() {
        let needles = watchlist_needles(r"https://www\.example\.com/abc123qzx", View::Raw).unwrap();
        assert_eq!(needles.len(), 3);
        assert_eq!(needles[0], "123");
        assert!(
            !needles
                .iter()
                .any(|s| s.contains('.') || s == "com" || s == "htt")
        );
        assert_eq!(
            watchlist_needles("abc.*abc", View::Raw).unwrap(),
            vec!["abc"]
        );
    }

    #[test]
    fn watchlist_needles_preserve_matches_on_hostile_bytes_and_views() {
        for view in View::ALL {
            for pattern in [
                "abc",
                "abc.*def.*ghi.*jkl",
                "abc.*def",
                "a%b",
                "a'b",
                r"a\\b",
                "(?i)abc",
                "é",
                "^abc$",
                "abc|xyz",
            ] {
                let matcher = Matcher::regex(pattern, view, false).unwrap();
                let mut counts = GramFileCounts::new();
                for (i, gram) in grams::literal_grams(view, b"abcdefghijkl", false, false)
                    .into_iter()
                    .enumerate()
                {
                    counts.insert(gram, (10 - i) as u64);
                }
                let needles: Vec<_> = [None, Some(&counts)]
                    .into_iter()
                    .flat_map(|counts| {
                        watchlist_needles_with_frequencies(pattern, view, counts).unwrap()
                    })
                    .collect();
                for value in [
                    b"\0\xffabcdefghijkl".as_slice(),
                    b"ABC\0def",
                    b"a%ba'ba\\b",
                    b"https://abc.def/",
                    b"%61bc%00def",
                    b"",
                    b"abc",
                ] {
                    if matcher.is_match(value) {
                        let transformed = view.apply(value);
                        for needle in &needles {
                            assert!(
                                transformed
                                    .windows(needle.len())
                                    .any(|s| s == needle.as_bytes()),
                                "{pattern:?} {view:?} {value:?} {needle:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn unsupported_or_and_exhaustion_widen_instead_of_excluding() {
        let a = GramQuery::Gram(grams::key(View::Raw, 97, 98, 99));
        let anchor = GramQuery::Gram(grams::key(View::Raw, grams::START, 97, 98));
        assert!(
            emit(
                "v",
                &GramQuery::Or(vec![a.clone(), anchor.clone()]),
                0,
                &mut 64
            )
            .is_none()
        );
        assert!(emit("v", &GramQuery::And(vec![a.clone(), anchor]), 0, &mut 64).is_some());
        assert!(emit("v", &GramQuery::Or(vec![a.clone(), a]), 0, &mut 2).is_none());
    }
}
