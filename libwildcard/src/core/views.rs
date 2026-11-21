// SPDX-License-Identifier: Apache-2.0
//! Views: deterministic transformations of a raw value.
//!
//! Security data is adversarial and unpredictable, so the stored value is never
//! normalised. Instead each index can add grams for derived views, and a query
//! chooses which view it searches. Verification applies the same transform to
//! the raw value, so results are always defined against the evidence.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum View {
    /// Bytes exactly as stored.
    Raw = 0,
    /// ASCII lower-case (byte-for-byte; non-ASCII bytes unchanged).
    Lower = 1,
    /// Percent-decoded, then ASCII lower-case. Catches `%2e%2e%2f`-style evasion.
    Decoded = 2,
    /// Host (from a URL or bare domain), lower-cased, trailing dot removed,
    /// labels reversed: `a.evil.com` -> `com.evil.a`. Makes `*.evil.com` a prefix.
    RevDomain = 3,
    /// Windows search representation: ASCII lower-case and backslashes to slashes.
    WinPath = 4,
}

impl View {
    pub const ALL: [View; 5] = [
        View::Raw,
        View::Lower,
        View::Decoded,
        View::RevDomain,
        View::WinPath,
    ];

    pub fn apply(self, v: &[u8]) -> Cow<'_, [u8]> {
        match self {
            View::Raw => Cow::Borrowed(v),
            View::Lower => Cow::Owned(v.to_ascii_lowercase()),
            View::Decoded => Cow::Owned(percent_decode(v).to_ascii_lowercase()),
            View::RevDomain => Cow::Owned(reverse_domain(v)),
            View::WinPath => Cow::Owned(
                v.iter()
                    .map(|b| {
                        if *b == b'\\' {
                            b'/'
                        } else {
                            b.to_ascii_lowercase()
                        }
                    })
                    .collect(),
            ),
        }
    }

    /// Transform wildcard/LIKE literal pieces after parsing their escapes.
    /// Decoded/domain patterns are already written in their derived form.
    pub fn pattern_literal(self, value: &[u8]) -> Vec<u8> {
        if self == View::WinPath {
            self.apply(value).into_owned()
        } else if self.folds_case() {
            value.to_ascii_lowercase()
        } else {
            value.to_vec()
        }
    }

    /// Whether pattern literals must be ASCII lower-cased for this view.
    pub fn folds_case(self) -> bool {
        !matches!(self, View::Raw)
    }

    pub fn name(self) -> &'static str {
        match self {
            View::Raw => "raw",
            View::Lower => "lower",
            View::Decoded => "decoded",
            View::RevDomain => "revdomain",
            View::WinPath => "winpath",
        }
    }

    pub fn from_u8(b: u8) -> Option<View> {
        View::ALL.get(b as usize).copied()
    }
}

impl fmt::Display for View {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for View {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        View::ALL
            .into_iter()
            .find(|v| v.name().eq_ignore_ascii_case(s))
            .ok_or_else(|| {
                format!("unknown view '{s}' (expected raw, lower, decoded, revdomain or winpath)")
            })
    }
}

/// Decode `%XX` sequences exactly like ClickHouse `decodeURLComponent`:
/// a `%` followed by two hex digits is decoded; a `%` followed by two other
/// characters is copied together with them (so `%4%41` stays `%4%41`); a `%`
/// with fewer than two characters after it is copied. `+` is kept.
pub fn percent_decode(v: &[u8]) -> Vec<u8> {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(v.len());
    let mut i = 0;
    while i < v.len() {
        if v[i] == b'%' && i + 2 < v.len() {
            match (hex(v[i + 1]), hex(v[i + 2])) {
                (Some(h), Some(l)) => out.push(h << 4 | l),
                _ => out.extend_from_slice(&v[i..i + 3]),
            }
            i += 3;
            continue;
        }
        out.push(v[i]);
        i += 1;
    }
    out
}

/// Extract the host from a URL (`scheme://[userinfo@]host[:port]/...`), or
/// return the whole value when it has no scheme. IPv6 literals keep brackets.
pub fn extract_host(v: &[u8]) -> &[u8] {
    let rest = match find(v, b"://") {
        Some(i) => &v[i + 3..],
        None => v,
    };
    let end = rest
        .iter()
        .position(|&b| matches!(b, b'/' | b'?' | b'#'))
        .unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = match authority.iter().rposition(|&b| b == b'@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    if host.first() == Some(&b'[') {
        return match host.iter().position(|&b| b == b']') {
            Some(i) => &host[..=i],
            None => host,
        };
    }
    match host.iter().rposition(|&b| b == b':') {
        Some(i) if host[i + 1..].iter().all(u8::is_ascii_digit) => &host[..i],
        _ => host,
    }
}

/// `https://User@WWW.Evil.com.:8080/x` -> `com.evil.www`.
pub fn reverse_domain(v: &[u8]) -> Vec<u8> {
    let host = extract_host(v).to_ascii_lowercase();
    let host = host.strip_suffix(b".").unwrap_or(&host);
    if host.first() == Some(&b'[') {
        return host.to_vec(); // IPv6 literal: not a domain
    }
    let mut out = Vec::with_capacity(host.len());
    for (i, label) in host.rsplit(|&b| b == b'.').enumerate() {
        if i > 0 {
            out.push(b'.');
        }
        out.extend_from_slice(label);
    }
    out
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode() {
        assert_eq!(percent_decode(b"%2e%2E%2fetc"), b"../etc");
        assert_eq!(percent_decode(b"100%"), b"100%");
        assert_eq!(percent_decode(b"%zz%4"), b"%zz%4");
        assert_eq!(percent_decode(b"%4%41"), b"%4%41");
        assert_eq!(percent_decode(b"%%41"), b"%%41");
        assert_eq!(percent_decode(b"%4G%41"), b"%4GA");
        assert_eq!(percent_decode(b"%25%41"), b"%A");
        assert_eq!(percent_decode(b"a+b"), b"a+b");
        assert_eq!(
            View::Decoded.apply(b"%2E%2E/WP-ADMIN").as_ref(),
            b"../wp-admin"
        );
    }

    #[test]
    fn hosts() {
        assert_eq!(
            extract_host(b"https://u:p@Host.example.com:8443/a?b#c"),
            b"Host.example.com"
        );
        assert_eq!(extract_host(b"evil.com"), b"evil.com");
        assert_eq!(extract_host(b"http://[::1]:80/"), b"[::1]");
        assert_eq!(extract_host(b"evil.com/path"), b"evil.com");
        assert_eq!(extract_host(b"host:notaport"), b"host:notaport");
    }

    #[test]
    fn reversed() {
        assert_eq!(
            reverse_domain(b"https://WWW.Evil.com.:8080/x"),
            b"com.evil.www"
        );
        assert_eq!(reverse_domain(b"localhost"), b"localhost");
        assert_eq!(reverse_domain(b"http://[::1]/"), b"[::1]");
        assert_eq!(reverse_domain(b""), b"");
    }

    #[test]
    fn parse_views() {
        assert_eq!("Decoded".parse::<View>(), Ok(View::Decoded));
        assert!("nope".parse::<View>().is_err());
    }
}

#[cfg(test)]
mod winpath_tests {
    use super::*;
    #[test]
    fn winpath_preserves_evidence_and_unc_structure() {
        let view: View = "winpath".parse().expect("Windows search view");
        let raw = br"C:\Windows\System32\CMD.EXE";
        assert_eq!(view.apply(raw).as_ref(), b"c:/windows/system32/cmd.exe");
        assert_eq!(raw, br"C:\Windows\System32\CMD.EXE");
        assert_eq!(
            view.apply(br"\\Server\Share\..\A").as_ref(),
            b"//server/share/../a"
        );
        assert_eq!(view.apply(b"C:\\A\0\xff").as_ref(), b"c:/a\0\xff");
    }
}
