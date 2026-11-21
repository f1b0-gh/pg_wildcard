// SPDX-License-Identifier: Apache-2.0
//! Emit bounded native candidate-profile cases; no service dependency.
use libwildcard::{
    antalya::{candidate_predicate, predicate},
    core::{Kind, View},
};
fn main() {
    let cases = [
        ("powershell", Kind::Wildcard, "*powershell*"),
        ("absence", Kind::Wildcard, "*never-present-malware*"),
        (
            "alternation",
            Kind::Regex {
                case_insensitive: false,
            },
            "powershell|cmd[.]exe",
        ),
        (
            "cross_boundary",
            Kind::Regex {
                case_insensitive: false,
            },
            "ab(cd|ef)gh",
        ),
        (
            "short_class",
            Kind::Regex {
                case_insensitive: false,
            },
            "a[bc]d",
        ),
        ("broad", Kind::Wildcard, "*"),
    ];
    let out: Vec<_> = cases
        .into_iter()
        .map(|(name, kind, pattern)| {
            serde_json::json!({
                "name": name,
                "exact": predicate("v", kind, pattern, View::Raw).unwrap(),
                "candidate": candidate_predicate("v", kind, pattern, View::Raw).unwrap(),
            })
        })
        .collect();
    println!("{}", serde_json::to_string(&out).unwrap());
}
