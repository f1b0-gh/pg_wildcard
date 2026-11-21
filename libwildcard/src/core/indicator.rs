// SPDX-License-Identifier: Apache-2.0
//! Indicator (IOC) hygiene: re-fanging, classification and a canonical match
//! spec. The **indicator** is normalised; stored data never is.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use crate::core::views::View;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndicatorType {
    Domain,
    Url,
    Ipv4,
    Ipv6,
    Md5,
    Sha1,
    Sha256,
    Email,
    Other,
}

impl fmt::Display for IndicatorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            IndicatorType::Domain => "domain",
            IndicatorType::Url => "url",
            IndicatorType::Ipv4 => "ipv4",
            IndicatorType::Ipv6 => "ipv6",
            IndicatorType::Md5 => "md5",
            IndicatorType::Sha1 => "sha1",
            IndicatorType::Sha256 => "sha256",
            IndicatorType::Email => "email",
            IndicatorType::Other => "other",
        })
    }
}

/// How to search for an indicator: a view plus an RE2 regex (usable in
/// ClickHouse `match` / `multiMatch*` and by [`crate::core::Matcher::regex`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub itype: IndicatorType,
    /// Canonical indicator value (re-fanged, normalised).
    pub value: String,
    pub view: View,
    pub regex: String,
}

/// Undo common defanging: `hxxp://evil[.]com`, `user[at]evil(.)com`, `1.2.3[.]4`.
pub fn refang(s: &str) -> String {
    const PAIRS: &[(&str, &str)] = &[
        ("[://]", "://"),
        ("[:]", ":"),
        ("[.]", "."),
        ("(.)", "."),
        ("{.}", "."),
        ("[dot]", "."),
        ("(dot)", "."),
        ("[@]", "@"),
        ("[at]", "@"),
        ("(at)", "@"),
        ("[/]", "/"),
    ];
    let mut out = s.trim().to_string();
    for (from, to) in PAIRS {
        out = replace_ci(&out, from, to);
    }
    for (from, to) in [
        ("hxxps://", "https://"),
        ("hxxp://", "http://"),
        ("hxxps[:]//", "https://"),
        ("fxp://", "ftp://"),
    ] {
        out = replace_ci(&out, from, to);
    }
    out
}

fn replace_ci(s: &str, from: &str, to: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(p) = lower[i..].find(from) {
        out.push_str(&s[i..i + p]);
        out.push_str(to);
        i += p + from.len();
    }
    out.push_str(&s[i..]);
    out
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn looks_like_domain(s: &str) -> bool {
    let s = s.strip_suffix('.').unwrap_or(s);
    let labels: Vec<&str> = s.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        })
        && labels
            .last()
            .is_some_and(|t| t.chars().any(|c| c.is_alphabetic()) || t.starts_with("xn--"))
}

pub fn classify(raw: &str) -> IndicatorType {
    let s = refang(raw);
    if s.parse::<Ipv4Addr>().is_ok() {
        return IndicatorType::Ipv4;
    }
    if s.trim_matches(|c| c == '[' || c == ']')
        .parse::<Ipv6Addr>()
        .is_ok()
    {
        return IndicatorType::Ipv6;
    }
    if is_hex(&s) {
        match s.len() {
            32 => return IndicatorType::Md5,
            40 => return IndicatorType::Sha1,
            64 => return IndicatorType::Sha256,
            _ => {}
        }
    }
    if s.contains("://") {
        return IndicatorType::Url;
    }
    if let Some((local, dom)) = s.split_once('@')
        && !local.is_empty()
        && looks_like_domain(dom)
    {
        return IndicatorType::Email;
    }
    if looks_like_domain(&s) {
        return IndicatorType::Domain;
    }
    IndicatorType::Other
}

/// IDNA to ASCII (punycode), lower-cased, trailing dot removed. Falls back to
/// ASCII lower-casing when the name is not valid IDNA.
pub fn normalize_domain(d: &str) -> String {
    let d = d.trim().trim_end_matches('.');
    idna::domain_to_ascii(d).unwrap_or_else(|_| d.to_ascii_lowercase())
}

/// Canonical spec for an indicator.
pub fn spec(raw: &str) -> Spec {
    let s = refang(raw);
    let itype = classify(&s);
    let esc = |t: &str| regex_syntax::escape(t);
    match itype {
        IndicatorType::Domain => {
            let value = normalize_domain(&s);
            let rev: Vec<&str> = value.rsplit('.').collect();
            // the domain itself and every subdomain, in the reversed-domain view
            Spec {
                itype,
                regex: format!("^{}(\\.|$)", esc(&rev.join("."))),
                view: View::RevDomain,
                value,
            }
        }
        IndicatorType::Ipv4 => Spec {
            itype,
            regex: format!("(^|[^0-9.]){}([^0-9]|$)", esc(&s)),
            view: View::Raw,
            value: s,
        },
        IndicatorType::Md5 | IndicatorType::Sha1 | IndicatorType::Sha256 => {
            let value = s.to_ascii_lowercase();
            Spec {
                itype,
                regex: format!("(^|[^0-9a-f]){}([^0-9a-f]|$)", esc(&value)),
                view: View::Lower,
                value,
            }
        }
        IndicatorType::Url => {
            // host part normalised; match case-insensitively on the decoded view
            let value = match s.split_once("://") {
                Some((scheme, rest)) => {
                    let (host, path) =
                        rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
                    format!(
                        "{}://{}{}",
                        scheme.to_ascii_lowercase(),
                        normalize_domain(host),
                        path
                    )
                }
                None => s.clone(),
            };
            let lowered = value.to_ascii_lowercase();
            Spec {
                itype,
                regex: esc(&lowered),
                view: View::Decoded,
                value,
            }
        }
        IndicatorType::Ipv6 | IndicatorType::Email | IndicatorType::Other => {
            let value = s.to_ascii_lowercase();
            Spec {
                itype,
                regex: esc(&value),
                view: View::Lower,
                value,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Matcher;

    #[test]
    fn refanging() {
        assert_eq!(refang("hxxp://evil[.]com/a"), "http://evil.com/a");
        assert_eq!(refang("HXXPS[://]Evil(.)com"), "https://Evil.com");
        assert_eq!(refang("bad[at]evil[dot]com"), "bad@evil.com");
        assert_eq!(refang("1.2.3[.]4"), "1.2.3.4");
    }

    #[test]
    fn classification() {
        assert_eq!(classify("evil[.]com"), IndicatorType::Domain);
        assert_eq!(classify("hxxp://evil.com/x"), IndicatorType::Url);
        assert_eq!(classify("10.0.0.1"), IndicatorType::Ipv4);
        assert_eq!(classify("[2001:db8::1]"), IndicatorType::Ipv6);
        assert_eq!(
            classify("d41d8cd98f00b204e9800998ecf8427e"),
            IndicatorType::Md5
        );
        assert_eq!(classify(&"a".repeat(64)), IndicatorType::Sha256);
        assert_eq!(classify("x@evil.com"), IndicatorType::Email);
        assert_eq!(classify("mimikatz.exe"), IndicatorType::Domain); // ambiguous by design: file names look like domains
        assert_eq!(classify("sekurlsa::logonpasswords"), IndicatorType::Other);
    }

    #[test]
    fn specs_match_what_they_should() {
        let s = spec("Login.Evil[.]com.");
        assert_eq!(s.value, "login.evil.com");
        let m = Matcher::regex(&s.regex, s.view, false).unwrap();
        assert!(m.is_match(b"https://a.LOGIN.evil.com/x"));
        assert!(m.is_match(b"login.evil.com"));
        assert!(!m.is_match(b"https://login.evil.com.attacker.net/"));
        assert!(!m.is_match(b"https://xlogin.evil.com/"));

        let s = spec("10.0.0.1");
        let m = Matcher::regex(&s.regex, s.view, false).unwrap();
        assert!(m.is_match(b"GET http://10.0.0.1:8080/"));
        assert!(!m.is_match(b"110.0.0.12"));

        let s = spec("bücher.example");
        assert_eq!(s.value, "xn--bcher-kva.example");

        let s = spec("hxxp://EVIL[.]com/Payload.exe");
        let m = Matcher::regex(&s.regex, s.view, false).unwrap();
        assert!(m.is_match(b"http://evil.com/%50ayload.EXE?x=1"));
    }
}
