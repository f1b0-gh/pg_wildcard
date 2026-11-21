// SPDX-License-Identifier: Apache-2.0
//! Small UTF-8 common-contract corpus; native timings are not a capacity benchmark.
use libwildcard::antalya::{ViewColumns, column_predicate};
use libwildcard::core::{
    View, grams,
    verify::{Kind, Matcher},
};
use serde_json::json;

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .map_or(1024, |s| s.parse().expect("row count"));
    assert!(
        (32..=10_000).contains(&count),
        "row count must be 32..=10000"
    );
    let seeds = [
        "powershell.exe -EncodedCommand AAA",
        r"C:\Windows\System32\cmd.exe",
        "cmd.exe /c whoami",
        "cmd.exe",
        "abefgh",
        "abcdgh",
        "abcXgh",
        "abd",
        "acd",
        "aad",
        "aed",
        "run0.exe",
        "run3.exe",
        "run9.exe",
        "GET /wp-admin/setup.php HTTP/1.1",
        "GET /wp-content/theme.php HTTP/1.1",
        "https://api.evil.example/login",
        "https://evil.example.attacker.invalid/",
        "Mozilla sqlmap/1.7",
        "Mozilla Chrome/120",
        "aéb",
        "ab",
        "aééb",
        "line1\nline2",
        "",
        r"\\SERVER\Share\tools\A.EXE",
        "%2e%2e/etc/passwd",
        "raw*literal?value",
        "ab3z",
        "xy7z",
        "abzz",
        "xy7zz",
    ];
    let values: Vec<String> = (0..count)
        .map(|i| {
            if i < seeds.len() {
                seeds[i].to_owned()
            } else {
                format!(
                    "event={i:06} {} trace={:016x}",
                    seeds[i % seeds.len()],
                    (i as u64).wrapping_mul(0x9e3779b97f4a7c15)
                )
            }
        })
        .collect();
    let views = [View::Raw, View::Lower, View::Decoded, View::WinPath];
    let documents: Vec<_> = values
        .iter()
        .enumerate()
        .map(|(id, v)| {
            let mut document = serde_json::Map::new();
            document.insert("id".into(), json!(id));
            for view in views {
                document.insert(
                    view.name().into(),
                    json!(
                        String::from_utf8(view.apply(v.as_bytes()).into_owned())
                            .expect("UTF-8 fixture")
                    ),
                );
            }
            json!(document)
        })
        .collect();
    let regex = Kind::Regex {
        case_insensitive: false,
    };
    let cases = [
        (
            "powershell",
            Kind::Wildcard,
            "*powershell*",
            "*powershell*",
            View::Lower,
        ),
        (
            "scanner",
            Kind::Wildcard,
            "*sqlmap/*",
            "*sqlmap/*",
            View::Lower,
        ),
        (
            "windows",
            Kind::Wildcard,
            "*/windows/system32/*.exe*",
            "*/windows/system32/*.exe*",
            View::WinPath,
        ),
        (
            "unc",
            Kind::Wildcard,
            "//server/share/*",
            "//server/share/*",
            View::WinPath,
        ),
        ("traversal", Kind::Wildcard, "*../*", "*../*", View::Decoded),
        ("utf8_one", Kind::Wildcard, "a?b", "a?b", View::Raw),
        ("all", Kind::Wildcard, "*", "*", View::Raw),
        ("empty", Kind::Wildcard, "", "", View::Raw),
        (
            "alternation_boundary",
            regex,
            "ab(cd|ef)gh",
            ".*ab(cd|ef)gh.*",
            View::Raw,
        ),
        ("small_class", regex, "a[bc]d", ".*a[bc]d.*", View::Raw),
        (
            "numbered_executable",
            regex,
            "run[0-3][.]exe",
            ".*run[0-3][.]exe.*",
            View::Raw,
        ),
        (
            "newline",
            regex,
            "line1.line2",
            ".*line1.line2.*",
            View::Raw,
        ),
        (
            "exact_executable",
            regex,
            "^cmd[.]exe$",
            "cmd[.]exe",
            View::Raw,
        ),
        (
            "shells",
            regex,
            "(powershell|cmd)[.]exe",
            ".*(powershell|cmd)[.]exe.*",
            View::Lower,
        ),
        (
            "anchored_class",
            regex,
            "^(ab|xy)[0-9]z$",
            "(ab|xy)[0-9]z",
            View::Raw,
        ),
    ];
    let columns = ViewColumns::new("v").unwrap();
    let indexed_columns = ViewColumns::conventional("v").unwrap();
    let cases: Vec<_> = cases.into_iter().map(|(name,kind,pattern,elastic_pattern,view)| {
        let matcher = Matcher::new(kind,pattern,view).unwrap();
        let expected: Vec<_> = values.iter().enumerate().filter_map(|(id,v)| matcher.is_match(v.as_bytes()).then_some(id)).collect();
        for &id in &expected {
            assert!(matcher.plan.eval_sorted(&grams::value_grams(view, &view.apply(values[id].as_bytes()))), "planner false negative: {name}, id {id}");
        }
        let candidates = values.iter().filter(|v| matcher.plan.eval_sorted(&grams::value_grams(view,&view.apply(v.as_bytes())))).count();
        json!({"name":name,"kind":kind.to_string(),"pattern":pattern,"elastic_pattern":elastic_pattern,"view":view.name(),"sql":column_predicate(&columns,kind,pattern,view).unwrap(),"indexed_sql":column_predicate(&indexed_columns,kind,pattern,view).unwrap(),"expected":expected,"candidate_count":candidates,"plan_grams":matcher.plan.grams().len()})
    }).collect();
    println!(
        "{}",
        json!({"schema":1,"rows":count,"documents":documents,"cases":cases,"native_indexed_ddl":libwildcard::antalya::view_columns_ddl("comparison","v",&views).unwrap(),"contract":"explicit paired whole-value Elastic and substring/anchored RE2 patterns; UTF-8; identical materialized views"})
    );
}
