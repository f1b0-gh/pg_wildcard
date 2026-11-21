// SPDX-License-Identifier: Apache-2.0
//! Byte trigrams with start/end anchors.
//!
//! A value `v` in view `V` becomes the symbol sequence `^ v $`, where `^` and
//! `$` are symbols outside the byte range (so NUL and any binary byte are
//! ordinary data). Every window of 3 symbols is a gram. A gram key packs the
//! view and three 9-bit symbols into a `u64`.

use crate::core::views::View;

/// Start-of-value symbol.
pub const START: u16 = 0x100;
/// End-of-value symbol.
pub const END: u16 = 0x101;

pub type GramKey = u64;

#[inline]
pub fn key(view: View, a: u16, b: u16, c: u16) -> GramKey {
    (view as u64) << 27 | (a as u64) << 18 | (b as u64) << 9 | c as u64
}

/// Decode a key into (view, symbols).
pub fn unpack(k: GramKey) -> (Option<View>, [u16; 3]) {
    let sym = |s: u32| ((k >> s) & 0x1ff) as u16;
    (View::from_u8((k >> 27) as u8), [sym(18), sym(9), sym(0)])
}

fn push_windows(view: View, seq: &[u16], out: &mut Vec<GramKey>) {
    for w in seq.windows(3) {
        out.push(key(view, w[0], w[1], w[2]));
    }
}

/// Anchored grams in value order, retaining duplicates and borrowing the bytes.
pub fn value_gram_iter(
    view: View,
    transformed: &[u8],
) -> impl ExactSizeIterator<Item = GramKey> + '_ {
    (0..transformed.len()).map(move |i| {
        let before = if i == 0 {
            START
        } else {
            u16::from(transformed[i - 1])
        };
        let after = transformed.get(i + 1).map_or(END, |&b| u16::from(b));
        key(view, before, u16::from(transformed[i]), after)
    })
}

/// All grams of an already-transformed value (call `view.apply` first).
/// Sorted and deduplicated.
pub fn value_grams(view: View, transformed: &[u8]) -> Vec<GramKey> {
    let mut out: Vec<_> = value_gram_iter(view, transformed).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Grams that any value containing `lit` must have. Anchors add the
/// start/end symbols when the literal must touch the value's edges.
pub fn literal_grams(view: View, lit: &[u8], at_start: bool, at_end: bool) -> Vec<GramKey> {
    let mut seq = Vec::with_capacity(lit.len() + 2);
    if at_start {
        seq.push(START);
    }
    seq.extend(lit.iter().map(|&b| b as u16));
    if at_end {
        seq.push(END);
    }
    let mut out = Vec::new();
    push_windows(view, &seq, &mut out);
    out.sort_unstable();
    out.dedup();
    out
}

/// Human-readable gram, e.g. `raw:"^ad"`.
pub fn display(k: GramKey) -> String {
    let (view, syms) = unpack(k);
    let mut s = String::new();
    for sym in syms {
        match sym {
            START => s.push('^'),
            END => s.push('$'),
            b if (0x20..0x7f).contains(&b) && b != b'"' as u16 && b != b'\\' as u16 => {
                s.push(b as u8 as char)
            }
            b => s.push_str(&format!("\\x{b:02x}")),
        }
    }
    format!("{}:\"{}\"", view.map_or("?", |v| v.name()), s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grams_of_short_values() {
        assert_eq!(value_grams(View::Raw, b"").len(), 0);
        assert_eq!(value_grams(View::Raw, b"a").len(), 1); // ^a$
        assert_eq!(value_grams(View::Raw, b"abc").len(), 3); // ^ab abc bc$
    }

    #[test]
    fn literal_grams_are_subset_of_value_grams() {
        let v = value_grams(View::Raw, b"GET /wp-admin/x.php");
        for g in literal_grams(View::Raw, b"wp-admin", false, false) {
            assert!(v.binary_search(&g).is_ok());
        }
        for g in literal_grams(View::Raw, b"GET", true, false) {
            assert!(v.binary_search(&g).is_ok());
        }
        for g in literal_grams(View::Raw, b".php", false, true) {
            assert!(v.binary_search(&g).is_ok());
        }
    }

    #[test]
    fn binary_bytes_and_views_are_distinct() {
        let a = key(View::Raw, 0, 0, 0);
        let b = key(View::Lower, 0, 0, 0);
        assert_ne!(a, b);
        assert_eq!(unpack(a).1, [0, 0, 0]);
        assert_eq!(
            display(key(View::Raw, START, b'a' as u16, 0)),
            "raw:\"^a\\x00\""
        );
    }
    #[test]
    fn streaming_grams_equal_independent_anchored_windows() {
        let mut values = vec![
            vec![],
            vec![0],
            vec![255],
            vec![0, 255],
            b"aaa".to_vec(),
            b"%4%41%zz%".to_vec(),
            b"user@xn--evil.test:443".to_vec(),
            b"https://[::1]/x".to_vec(),
            (0..=255).collect::<Vec<u8>>(),
        ];
        let mut state = 42u64;
        for len in [3, 7, 31, 256, 8192, 100_000] {
            values.push(
                (0..len)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        state as u8
                    })
                    .collect(),
            );
        }
        for value in values {
            for view in View::ALL {
                let transformed = view.apply(&value);
                let symbols: Vec<u16> = std::iter::once(START)
                    .chain(transformed.iter().map(|&b| u16::from(b)))
                    .chain(std::iter::once(END))
                    .collect();
                let expected: Vec<_> = symbols
                    .windows(3)
                    .map(|w| key(view, w[0], w[1], w[2]))
                    .collect();
                let mut actual = value_gram_iter(view, &transformed);
                assert_eq!(actual.len(), expected.len());
                for (i, gram) in expected.iter().enumerate() {
                    assert_eq!(actual.next(), Some(*gram));
                    assert_eq!(actual.len(), expected.len() - i - 1);
                }
                assert_eq!(actual.next(), None);
                assert_eq!(actual.next(), None);
                let mut sorted = expected;
                sorted.sort_unstable();
                sorted.dedup();
                assert_eq!(value_grams(view, &transformed), sorted);
            }
        }
    }
}
