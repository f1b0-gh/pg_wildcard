// SPDX-License-Identifier: Apache-2.0
//! Boolean gram queries: the candidate filter.

use std::fmt;

use crate::core::grams::{self, GramKey};

// Structural order is deterministic and keeps byte/anchor identities distinct;
// display strings are neither allocation-free nor unique keys.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GramQuery {
    /// No filtering possible: every row is a candidate (planner should prefer a seq scan).
    All,
    Gram(GramKey),
    And(Vec<GramQuery>),
    Or(Vec<GramQuery>),
}

impl GramQuery {
    /// AND of all grams (empty -> All).
    pub fn all_of(grams: Vec<GramKey>) -> Self {
        Self::and(grams.into_iter().map(GramQuery::Gram).collect())
    }

    pub fn and(parts: Vec<GramQuery>) -> Self {
        let mut out = Vec::new();
        for p in parts {
            match p {
                GramQuery::All => {}
                GramQuery::And(inner) => out.extend(inner),
                other => out.push(other),
            }
        }
        out.sort_unstable();
        out.dedup();
        match out.len() {
            0 => GramQuery::All,
            1 => out.into_iter().next().unwrap_or(GramQuery::All),
            _ => GramQuery::And(out),
        }
    }

    pub fn or(parts: Vec<GramQuery>) -> Self {
        if parts.is_empty() || parts.iter().any(|p| matches!(p, GramQuery::All)) {
            return GramQuery::All;
        }
        let mut out = Vec::new();
        for p in parts {
            match p {
                GramQuery::Or(inner) => out.extend(inner),
                other => out.push(other),
            }
        }
        out.sort_unstable();
        out.dedup();
        match out.len() {
            1 => out.into_iter().next().unwrap_or(GramQuery::All),
            _ => GramQuery::Or(out),
        }
    }

    pub fn is_all(&self) -> bool {
        matches!(self, GramQuery::All)
    }

    /// Evaluate against a value's gram set.
    pub fn eval(&self, has: &impl Fn(GramKey) -> bool) -> bool {
        match self {
            GramQuery::All => true,
            GramQuery::Gram(g) => has(*g),
            GramQuery::And(v) => v.iter().all(|q| q.eval(has)),
            GramQuery::Or(v) => v.iter().any(|q| q.eval(has)),
        }
    }

    /// Evaluate against a sorted gram list.
    pub fn eval_sorted(&self, grams: &[GramKey]) -> bool {
        self.eval(&|g| grams.binary_search(&g).is_ok())
    }

    /// Evaluate over posting sets: `lookup(gram)` returns the sorted ids (for
    /// example file ordinals) containing the gram; `universe` is the number of
    /// ids. Returns the sorted candidate ids.
    pub fn candidates(
        &self,
        universe: u32,
        lookup: &mut impl FnMut(GramKey) -> Vec<u32>,
    ) -> Vec<u32> {
        match self {
            GramQuery::All => (0..universe).collect(),
            GramQuery::Gram(g) => lookup(*g),
            GramQuery::And(v) => {
                let mut acc: Option<Vec<u32>> = None;
                for q in v {
                    let s = q.candidates(universe, lookup);
                    acc = Some(match acc {
                        None => s,
                        Some(a) => intersect(&a, &s),
                    });
                    if acc.as_ref().is_some_and(|a| a.is_empty()) {
                        break;
                    }
                }
                acc.unwrap_or_else(|| (0..universe).collect())
            }
            GramQuery::Or(v) => {
                let mut acc = Vec::new();
                for q in v {
                    acc = union(&acc, &q.candidates(universe, lookup));
                }
                acc
            }
        }
    }

    /// All distinct grams referenced.
    pub fn grams(&self) -> Vec<GramKey> {
        fn walk(q: &GramQuery, out: &mut Vec<GramKey>) {
            match q {
                GramQuery::All => {}
                GramQuery::Gram(g) => out.push(*g),
                GramQuery::And(v) | GramQuery::Or(v) => v.iter().for_each(|q| walk(q, out)),
            }
        }
        let mut out = Vec::new();
        walk(self, &mut out);
        out.sort_unstable();
        out.dedup();
        out
    }
}

impl fmt::Display for GramQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GramQuery::All => f.write_str("ALL"),
            GramQuery::Gram(g) => f.write_str(&grams::display(*g)),
            GramQuery::And(v) | GramQuery::Or(v) => {
                f.write_str(if matches!(self, GramQuery::And(_)) {
                    "AND("
                } else {
                    "OR("
                })?;
                for (i, q) in v.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{q}")?;
                }
                f.write_str(")")
            }
        }
    }
}

fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

fn union(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    out.extend_from_slice(a);
    out.extend_from_slice(b);
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalization_preserves_display_collisions_and_ignores_input_order() {
        use crate::core::views::View;
        let anchor = grams::key(View::Raw, grams::START, b'a' as u16, b'b' as u16);
        let literal = grams::key(View::Raw, b'^' as u16, b'a' as u16, b'b' as u16);
        assert_eq!(grams::display(anchor), grams::display(literal));
        assert_ne!(anchor, literal);
        for combine in [GramQuery::and, GramQuery::or] {
            let a = combine(vec![
                GramQuery::Gram(anchor),
                GramQuery::Gram(anchor),
                GramQuery::Gram(literal),
            ]);
            let b = combine(vec![
                GramQuery::Gram(literal),
                GramQuery::Gram(anchor),
                GramQuery::Gram(anchor),
            ]);
            assert_eq!(a, b, "same logical operands must have one canonical order");
            assert_eq!(
                a.grams().len(),
                2,
                "distinct anchor and literal must survive"
            );
        }
    }

    #[test]
    fn canonicalization_preserves_small_truth_tables() {
        fn next(state: &mut u64) -> u64 {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            *state
        }
        fn tree(state: &mut u64, depth: usize) -> GramQuery {
            if depth == 0 {
                return match next(state) % 6 {
                    5 => GramQuery::All,
                    n => GramQuery::Gram(n),
                };
            }
            let parts = (0..1 + next(state) % 4)
                .map(|_| tree(state, depth - 1))
                .collect();
            if next(state).is_multiple_of(2) {
                GramQuery::And(parts)
            } else {
                GramQuery::Or(parts)
            }
        }
        fn normalize(q: GramQuery, reverse: bool) -> GramQuery {
            match q {
                GramQuery::And(mut parts) => {
                    if reverse {
                        parts.reverse();
                    }
                    GramQuery::and(parts.into_iter().map(|p| normalize(p, reverse)).collect())
                }
                GramQuery::Or(mut parts) => {
                    if reverse {
                        parts.reverse();
                    }
                    GramQuery::or(parts.into_iter().map(|p| normalize(p, reverse)).collect())
                }
                other => other,
            }
        }
        let mut seed = 0x2026_1115_0055_1234;
        for _ in 0..300 {
            let raw = tree(&mut seed, 3);
            let normalized = normalize(raw.clone(), false);
            assert_eq!(normalized, normalize(raw.clone(), true));
            for bits in 0..32 {
                let present = |gram| bits & (1 << gram) != 0;
                assert_eq!(raw.eval(&present), normalized.eval(&present));
            }
        }
    }

    #[test]
    fn simplification() {
        assert_eq!(GramQuery::and(vec![]), GramQuery::All);
        assert_eq!(
            GramQuery::and(vec![GramQuery::All, GramQuery::Gram(1)]),
            GramQuery::Gram(1)
        );
        assert_eq!(
            GramQuery::or(vec![GramQuery::Gram(1), GramQuery::All]),
            GramQuery::All
        );
        let q = GramQuery::and(vec![
            GramQuery::Gram(2),
            GramQuery::and(vec![GramQuery::Gram(1), GramQuery::Gram(3)]),
        ]);
        assert_eq!(q.grams(), vec![1, 2, 3]);
        assert!(q.eval_sorted(&[1, 2, 3, 9]));
        assert!(!q.eval_sorted(&[1, 2]));
    }

    #[test]
    fn set_candidates() {
        let q = GramQuery::or(vec![
            GramQuery::and(vec![GramQuery::Gram(1), GramQuery::Gram(2)]),
            GramQuery::Gram(3),
        ]);
        let mut lk = |g: u64| match g {
            1 => vec![0, 1, 4],
            2 => vec![1, 4, 5],
            3 => vec![7],
            _ => vec![],
        };
        assert_eq!(q.candidates(10, &mut lk), vec![1, 4, 7]);
        assert_eq!(GramQuery::All.candidates(3, &mut lk), vec![0, 1, 2]);
    }
}
