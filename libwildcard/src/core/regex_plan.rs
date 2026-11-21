// SPDX-License-Identifier: Apache-2.0
//! Regex -> gram query.
//!
//! A sound analysis over `regex-syntax`'s HIR: inside each
//! concatenation, maximal runs of literal bytes become AND-ed literal grams;
//! alternations become OR; optional parts and classes become `All`. It never
//! produces false negatives. Bounded prefix/suffix extraction adds grams across
//! finite alternatives and small classes; the original analysis remains the
//! fallback when extraction cannot provide a useful necessary condition.

use regex_syntax::ParserBuilder;
use regex_syntax::hir::literal::{ExtractKind, Extractor};
use regex_syntax::hir::{Class, Hir, HirKind, Look};
use thiserror::Error;

use crate::core::grams;
use crate::core::query::GramQuery;
use crate::core::views::View;

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("invalid regex: {0}")]
    Syntax(String),
}

/// Plan a regex searched (unanchored, like Postgres `~`) in `view`.
pub fn plan_regex(
    pattern: &str,
    view: View,
    case_insensitive: bool,
) -> Result<GramQuery, PlanError> {
    let hir = ParserBuilder::new()
        .utf8(false)
        .case_insensitive(case_insensitive)
        .build()
        .parse(pattern)
        .map_err(|e| PlanError::Syntax(e.to_string()))?;
    Ok(GramQuery::and(vec![
        walk(&hir, view),
        boundary_query(&hir, view, ExtractKind::Prefix),
        boundary_query(&hir, view, ExtractKind::Suffix),
    ]))
}

/// Each match contains at least one of the extracted literals, including when
/// extraction shortens them to fit its limits. Keep ALL alternatives: dropping
/// an OR branch would be unsound. Empty/short literals naturally widen to All.
/// No edge grams are inferred here: a match boundary is not a value boundary.
fn boundary_query(hir: &Hir, view: View, kind: ExtractKind) -> GramQuery {
    let seq = Extractor::new()
        .kind(kind)
        .limit_class(16)
        .limit_repeat(4)
        .limit_literal_len(64)
        .limit_total(32)
        .extract(hir);
    let Some(literals) = seq.literals() else {
        return GramQuery::All;
    };
    // At most 128 gram occurrences per direction (256 additional overall).
    // This bounds the added posting work, not the legacy plan's size.
    if literals
        .iter()
        .map(|l| l.as_bytes().len().saturating_sub(2))
        .sum::<usize>()
        > 128
    {
        return GramQuery::All;
    }
    GramQuery::or(
        literals
            .iter()
            .map(|l| lit_query(view, l.as_bytes(), false, false))
            .collect(),
    )
}

/// A single-position literal: plain literal bytes, or (in case-folding views)
/// a class that is exactly the upper/lower pair of one ASCII letter.
fn literal_bytes(h: &Hir, view: View) -> Option<Vec<u8>> {
    match h.kind() {
        HirKind::Literal(l) => Some(l.0.to_vec()),
        HirKind::Class(c) if view.folds_case() => {
            let pair: Vec<(u32, u32)> = match c {
                Class::Unicode(u) => u
                    .ranges()
                    .iter()
                    .map(|r| (r.start() as u32, r.end() as u32))
                    .collect(),
                Class::Bytes(b) => b
                    .ranges()
                    .iter()
                    .map(|r| (r.start() as u32, r.end() as u32))
                    .collect(),
            };
            let chars: Vec<u32> = pair.iter().flat_map(|&(a, b)| a..=b).take(3).collect();
            match chars.as_slice() {
                [a, b] if *a < 128 && *b < 128 && (*a as u8).eq_ignore_ascii_case(&(*b as u8)) => {
                    Some(vec![(*a as u8).to_ascii_lowercase()])
                }
                [a] if *a < 128 => Some(vec![*a as u8]),
                _ => None,
            }
        }
        _ => None,
    }
}

fn lit_query(view: View, buf: &[u8], at_start: bool, at_end: bool) -> GramQuery {
    let lit = if view.folds_case() {
        buf.to_ascii_lowercase()
    } else {
        buf.to_vec()
    };
    GramQuery::all_of(grams::literal_grams(view, &lit, at_start, at_end))
}

fn walk(h: &Hir, view: View) -> GramQuery {
    match h.kind() {
        HirKind::Empty | HirKind::Look(_) | HirKind::Class(_) => match literal_bytes(h, view) {
            Some(b) => lit_query(view, &b, false, false),
            None => GramQuery::All,
        },
        HirKind::Literal(l) => lit_query(view, &l.0, false, false),
        HirKind::Repetition(r) if r.min == 0 => GramQuery::All,
        HirKind::Repetition(r) => walk(&r.sub, view),
        HirKind::Capture(c) => walk(&c.sub, view),
        HirKind::Alternation(subs) => GramQuery::or(subs.iter().map(|s| walk(s, view)).collect()),
        HirKind::Concat(subs) => {
            let mut parts = Vec::new();
            let mut buf: Vec<u8> = Vec::new();
            let mut buf_at_start = false;
            // True only right after `\A`: regex search is unanchored by default.
            let mut at_text_start = false;
            for s in subs {
                if let Some(b) = literal_bytes(s, view) {
                    if buf.is_empty() {
                        buf_at_start = at_text_start;
                    }
                    buf.extend(b);
                    continue;
                }
                match s.kind() {
                    HirKind::Look(Look::Start) => {
                        // `\A` (or `^` without multi-line): following literal touches the start
                        if buf.is_empty() {
                            at_text_start = true;
                            continue;
                        }
                    }
                    HirKind::Look(Look::End) => {
                        if !buf.is_empty() {
                            parts.push(lit_query(view, &buf, buf_at_start, true));
                            buf.clear();
                        }
                        continue;
                    }
                    _ => {}
                }
                if !buf.is_empty() {
                    parts.push(lit_query(view, &buf, buf_at_start, false));
                    buf.clear();
                }
                parts.push(walk(s, view));
                at_text_start = false;
            }
            if !buf.is_empty() {
                parts.push(lit_query(view, &buf, buf_at_start, false));
            }
            GramQuery::and(parts)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Diagnostic only: same parsed HIR, legacy analysis versus bounded analysis.
    /// Run optimized, --ignored --nocapture. No wall-clock acceptance threshold.
    #[test]
    #[ignore = "manual planner-cost microbenchmark"]
    fn planning_cost_diagnostic() {
        use std::hint::black_box;
        use std::time::Instant;
        for pattern in [
            "ab(cd|ef)gh",
            "a[bc]d",
            "run[0-3][.]exe",
            "^(ab|xy)[0-9]z$",
            "powershell.*encodedcommand",
            "[a-z]{6}",
        ] {
            let hir = ParserBuilder::new().build().parse(pattern).unwrap();
            for round in 0..4 {
                for bounded in if round % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let start = Instant::now();
                    for _ in 0..2000 {
                        let old = walk(black_box(&hir), View::Raw);
                        let plan = if bounded {
                            GramQuery::and(vec![
                                old,
                                boundary_query(&hir, View::Raw, ExtractKind::Prefix),
                                boundary_query(&hir, View::Raw, ExtractKind::Suffix),
                            ])
                        } else {
                            old
                        };
                        black_box(plan);
                    }
                    println!(
                        "pattern={pattern:?} bounded={bounded} round={round} ns_per_plan={}",
                        start.elapsed().as_nanos() / 2000
                    );
                }
            }
        }
    }

    fn p(re: &str, view: View, ci: bool) -> String {
        format!("{}", plan_regex(re, view, ci).unwrap())
    }

    #[test]
    fn literals_and_alternation() {
        let q = p(
            "powershell(\\.exe)? -enc(odedcommand)? [A-Za-z0-9+/=]{20,}",
            View::Raw,
            false,
        );
        assert!(
            q.contains("raw:\"pow\"") && q.contains("raw:\"-en\""),
            "{q}"
        );
        let q = p("(evil|bad)\\.com", View::Raw, false);
        assert!(q.contains("OR("), "{q}");
        assert_eq!(p(".*", View::Raw, false), "ALL");
        assert_ne!(p("a[bc]d", View::Raw, false), "ALL");
    }

    #[test]
    fn anchors() {
        let q = p("^GET /admin", View::Raw, false);
        assert!(q.contains("raw:\"^GE\""), "{q}");
        let q = p("\\.php$", View::Raw, false);
        assert!(q.contains("raw:\"hp$\""), "{q}");
    }

    #[test]
    fn case_insensitive_uses_folded_literals() {
        let q = p("CMD\\.EXE", View::Lower, true);
        assert!(q.contains("lower:\"cmd\""), "{q}");
        // Small raw-view case alternatives can now be enumerated safely.
        assert_ne!(p("cmd", View::Raw, true), "ALL");
    }

    #[test]
    fn rejects_bad_regex() {
        assert!(plan_regex("(", View::Raw, false).is_err());
    }

    #[test]
    fn finite_boundaries_reject_unrelated_candidates() {
        for (pattern, positives, negative) in [
            ("ab(cd|ef)gh", vec!["xxabcdgh", "abefgh"], "abxxgh"),
            ("a[bc]d", vec!["abd", "acd"], "aed"),
            ("run[0-3][.]exe", vec!["run0.exe", "run3.exe"], "run9.exe"),
            ("^(ab|xy)[0-9]z$", vec!["ab0z", "xy9z"], "qq1z"),
        ] {
            let q = plan_regex(pattern, View::Raw, false).unwrap();
            for value in positives {
                assert!(
                    q.eval_sorted(&grams::value_grams(View::Raw, value.as_bytes())),
                    "{pattern}: {value}"
                );
            }
            assert!(
                !q.eval_sorted(&grams::value_grams(View::Raw, negative.as_bytes())),
                "unfiltered {pattern}: {q}"
            );
        }
    }

    #[test]
    fn extraction_budget_preserves_the_existing_filter() {
        // 16 alternatives * >8 trigrams exceeds the added per-direction budget.
        let pattern = "[a-p]0123456789[q-z]";
        let hir = ParserBuilder::new().build().parse(pattern).unwrap();
        assert!(boundary_query(&hir, View::Raw, ExtractKind::Prefix).is_all());
        let plan = plan_regex(pattern, View::Raw, false).unwrap();
        assert!(!plan.is_all());
        for first in b'a'..=b'p' {
            for last in b'q'..=b'z' {
                let value = [vec![first], b"0123456789".to_vec(), vec![last]].concat();
                assert!(plan.eval_sorted(&grams::value_grams(View::Raw, &value)));
            }
        }
    }
}
