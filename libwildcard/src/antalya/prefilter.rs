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

/// Necessary byte substrings for a dynamic RE2 watchlist. Only a bounded pure
/// conjunction of printable ASCII interior trigrams is supported. At most one
/// lexicographically first necessary trigram is returned; selection has no
/// corpus-frequency statistics. This caps nested dynamic-array amplification. Empty means
/// exact matching only (including unsupported plans or exhausted budgets).
/// Validation is identical to the exact SQL API.
pub fn watchlist_needles(pattern: &str, view: View) -> Result<Vec<String>, ChError> {
    let kind = Kind::Regex {
        case_insensitive: false,
    };
    let _ = predicate("v", kind, pattern, view)?;
    if pattern.len() > 8192 {
        return Ok(Vec::new());
    }
    let Ok(matcher) = Matcher::new(kind, pattern, view) else {
        return Ok(Vec::new());
    };
    fn collect(query: &GramQuery, depth: usize, budget: &mut usize, out: &mut Vec<String>) -> bool {
        if depth >= 32 || *budget == 0 {
            return false;
        }
        *budget -= 1;
        match query {
            GramQuery::Gram(g) => {
                let (_, symbols) = grams::unpack(*g);
                if !symbols.iter().all(|s| (32..127).contains(s)) {
                    return false;
                }
                out.push(symbols.iter().map(|s| char::from(*s as u8)).collect());
                true
            }
            GramQuery::And(children) => children.iter().all(|q| collect(q, depth + 1, budget, out)),
            GramQuery::All | GramQuery::Or(_) => false,
        }
    }
    let mut needles = Vec::new();
    if !collect(&matcher.plan, 0, &mut 64, &mut needles) {
        needles.clear();
    }
    needles.sort();
    needles.dedup();
    needles.truncate(1);
    Ok(needles)
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
    #[test]
    fn watchlist_needles_require_bounded_safe_conjunctions() {
        assert_eq!(watchlist_needles("abc", View::Raw).unwrap(), vec!["abc"]);
        assert_eq!(
            watchlist_needles("abcdefgh", View::Raw).unwrap(),
            vec!["abc"]
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
    fn watchlist_needles_preserve_matches_on_hostile_bytes_and_views() {
        for view in View::ALL {
            for pattern in [
                "abc", "abc.*def", "a%b", "a'b", r"a\\b", "(?i)abc", "é", "^abc$", "abc|xyz",
            ] {
                let matcher = Matcher::regex(pattern, view, false).unwrap();
                let needles = watchlist_needles(pattern, view).unwrap();
                for value in [
                    b"\0\xffabcdef".as_slice(),
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
