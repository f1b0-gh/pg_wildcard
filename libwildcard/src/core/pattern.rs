// SPDX-License-Identifier: Apache-2.0
//! SQL `LIKE` and Elasticsearch-style wildcard patterns.
//!
//! Both match the **whole** value. They are parsed into [`Piece`]s, planned
//! into a [`GramQuery`] and compiled to an anchored regex for verification.

use crate::core::grams;
use crate::core::query::GramQuery;
use crate::core::views::View;

/// Source-pattern admission limit, checked before parsing or translation.
pub const MAX_PATTERN_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("pattern exceeds {MAX_PATTERN_BYTES} bytes")]
pub struct PatternTooLong;

pub fn check_pattern_len(pattern: &[u8]) -> Result<(), PatternTooLong> {
    if pattern.len() > MAX_PATTERN_BYTES {
        Err(PatternTooLong)
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Lit(Vec<u8>),
    /// Exactly one UTF-8 character (`_` / `?`), as in ClickHouse and Postgres.
    AnyOne,
    /// Any run of characters (`%` / `*`).
    AnyMany,
}

fn push_lit(out: &mut Vec<Piece>, b: u8) {
    match out.last_mut() {
        Some(Piece::Lit(v)) => v.push(b),
        _ => out.push(Piece::Lit(vec![b])),
    }
}

fn parse(p: &[u8], one: u8, many: u8, escape: Option<u8>) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < p.len() {
        let b = p[i];
        if Some(b) == escape && i + 1 < p.len() {
            push_lit(&mut out, p[i + 1]);
            i += 2;
            continue;
        }
        if b == many {
            if out.last() != Some(&Piece::AnyMany) {
                out.push(Piece::AnyMany);
            }
        } else if b == one {
            out.push(Piece::AnyOne);
        } else {
            push_lit(&mut out, b);
        }
        i += 1;
    }
    out
}

/// SQL LIKE: `%` any run, `_` one, `\` escape (Postgres default).
/// Low-level, unbounded parser; use [`parse_like_checked`] for untrusted input.
pub fn parse_like(p: &[u8]) -> Vec<Piece> {
    parse(p, b'_', b'%', Some(b'\\'))
}

/// Elasticsearch wildcard: `*` any run, `?` one, `\` escape.
/// Low-level, unbounded parser; use [`parse_wildcard_checked`] for untrusted input.
pub fn parse_wildcard(p: &[u8]) -> Vec<Piece> {
    parse(p, b'?', b'*', Some(b'\\'))
}

pub fn parse_like_checked(p: &[u8]) -> Result<Vec<Piece>, PatternTooLong> {
    check_pattern_len(p)?;
    Ok(parse_like(p))
}

pub fn parse_wildcard_checked(p: &[u8]) -> Result<Vec<Piece>, PatternTooLong> {
    check_pattern_len(p)?;
    Ok(parse_wildcard(p))
}

/// Candidate filter for a whole-value pattern searched in `view`.
pub fn plan(pieces: &[Piece], view: View) -> GramQuery {
    let n = pieces.len();
    let mut parts = Vec::new();
    for (i, p) in pieces.iter().enumerate() {
        if let Piece::Lit(lit) = p {
            let lit = view.pattern_literal(lit);
            parts.push(GramQuery::all_of(grams::literal_grams(
                view,
                &lit,
                i == 0,
                i + 1 == n,
            )));
        }
    }
    // An empty pattern matches only the empty value: grams cannot express that.
    GramQuery::and(parts)
}

/// Regex equivalent of the pattern with ClickHouse `LIKE` semantics, for an
/// **unanchored** search compiled with `unicode(true)` on `regex::bytes`:
/// - `_`/`?` match one UTF-8 character (newline included), `%`/`*` any run;
/// - a leading `%` removes the start anchor and a trailing `%` the end anchor
///   (ClickHouse does the same), so `%x%`, `a%` and `%b` can match values
///   containing invalid UTF-8, while `.`/`.*` never cross an invalid byte.
pub fn to_regex(pieces: &[Piece]) -> String {
    let start = pieces
        .iter()
        .position(|p| *p != Piece::AnyMany)
        .unwrap_or(pieces.len());
    let end = pieces
        .iter()
        .rposition(|p| *p != Piece::AnyMany)
        .map_or(start, |i| i + 1);
    let core = if start < end {
        &pieces[start..end]
    } else {
        &[][..]
    };
    let mut s = String::from("(?s)");
    if start == 0 {
        s.push_str(r"\A");
    }
    for p in core {
        match p {
            Piece::Lit(v) => match std::str::from_utf8(v) {
                Ok(t) => s.push_str(&regex_syntax::escape(t)),
                Err(_) => {
                    for &b in v {
                        if b.is_ascii() {
                            s.push_str(&regex_syntax::escape(&(b as char).to_string()));
                        } else {
                            s.push_str(&format!(r"(?-u:\x{b:02X})"));
                        }
                    }
                }
            },
            Piece::AnyOne => s.push('.'),
            Piece::AnyMany => s.push_str(".*"),
        }
    }
    if end == pieces.len() && !pieces.is_empty() || pieces.is_empty() {
        s.push_str(r"\z");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        assert_eq!(
            parse_like(b"%/wp\\_admin/_x%%"),
            vec![
                Piece::AnyMany,
                Piece::Lit(b"/wp_admin/".to_vec()),
                Piece::AnyOne,
                Piece::Lit(b"x".to_vec()),
                Piece::AnyMany
            ]
        );
        assert_eq!(
            parse_wildcard(b"*.evil.com"),
            vec![Piece::AnyMany, Piece::Lit(b".evil.com".to_vec())]
        );
    }

    #[test]
    fn checked_parsers_admit_the_boundary_and_reject_before_allocating_pieces() {
        let mut pattern = vec![b'_'; MAX_PATTERN_BYTES];
        assert_eq!(parse_like_checked(&pattern).unwrap(), parse_like(&pattern));
        assert_eq!(
            parse_wildcard_checked(&pattern).unwrap(),
            parse_wildcard(&pattern)
        );
        pattern.push(b'_');
        assert_eq!(parse_like_checked(&pattern), Err(PatternTooLong));
        assert_eq!(parse_wildcard_checked(&pattern), Err(PatternTooLong));
    }

    #[test]
    fn planning() {
        assert!(plan(&parse_like(b"%ab%"), View::Raw).is_all());
        let q = plan(&parse_like(b"cmd.exe%"), View::Lower);
        assert!(format!("{q}").contains("lower:\"^cm\""));
        let q = plan(&parse_wildcard(b"*.EXE"), View::Lower);
        assert!(format!("{q}").contains("lower:\"xe$\""));
    }
}
