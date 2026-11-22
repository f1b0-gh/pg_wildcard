// SPDX-License-Identifier: Apache-2.0
//! Versioned comparison fixtures. Never infer the oracle from generated SQL.
use libwildcard::antalya::{
    SweepItem, SweepOptions, ViewColumns, predicate, sweep_queries_with_options, view_columns_ddl,
    watchlist_insert_sql, watchlist_mv, watchlist_needles, watchlist_table_ddl,
};
use libwildcard::core::{
    GramQuery, View, grams,
    verify::{Kind, Matcher},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[path = "observables.rs"]
mod observables;

fn required_grams(query: &GramQuery) -> Vec<String> {
    fn collect(q: &GramQuery, budget: &mut usize, out: &mut Vec<String>) -> bool {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        match q {
            GramQuery::Gram(g) => {
                let (_, symbols) = grams::unpack(*g);
                if !symbols.iter().all(|s| (32..127).contains(s)) {
                    return false;
                }
                out.push(symbols.iter().map(|s| char::from(*s as u8)).collect());
                true
            }
            GramQuery::And(children) => children.iter().all(|q| collect(q, budget, out)),
            _ => false,
        }
    }
    let mut out = Vec::new();
    if !collect(query, &mut 64, &mut out) {
        return Vec::new();
    }
    out.sort();
    out.dedup();
    out
}

fn token(id: usize) -> String {
    let mut x = (id as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    format!("{:016x}", x ^ (x >> 31))
}

pub fn build(variant: &str, count: usize, rows: usize) -> Result<Value, String> {
    let family = observables::Family::parse(variant);
    if !(1..=10000).contains(&count)
        || !(1..=1000000).contains(&rows)
        || (family.is_none()
            && !["shared", "unrelated", "common", "short", "mixed"].contains(&variant))
    {
        return Err("invalid comparison fixture bounds or variant".into());
    }
    let mut items = Vec::new();
    let mut templates = Vec::new();
    let mut elastic = Vec::new();
    let mut near_misses = Vec::new();
    let mut shapes = Vec::new();
    for id in 0..count {
        let marker = token(id);
        let case = family.map(|f| f.case(id, &marker));
        shapes.push(case.as_ref().map(|c| c.shape));
        let (regex, value, wildcard, view) = if let Some(case) = case {
            use libwildcard::core::pattern::{parse_wildcard, to_regex};
            near_misses.push(case.negative);
            (
                to_regex(&parse_wildcard(case.wildcard.as_bytes())),
                case.positive,
                Some(case.wildcard),
                View::Raw,
            )
        } else {
            match variant {
                "shared" => (
                    format!("evil{id:04}\\.example"),
                    format!("https://evil{id:04}.example/path"),
                    Some(format!("*evil{id:04}.example*")),
                    View::Raw,
                ),
                "unrelated" => (
                    marker.clone(),
                    format!("https://{marker}.test/path"),
                    Some(format!("*{marker}*")),
                    View::Raw,
                ),
                "common" => {
                    let bits = format!("{id:014b}");
                    (
                        format!(
                            "common{}",
                            bits.chars().map(|c| format!(".{c}")).collect::<String>()
                        ),
                        format!(
                            "https://common{}/path",
                            bits.chars().map(|c| format!("/{c}")).collect::<String>()
                        ),
                        Some(format!(
                            "*common{}*",
                            bits.chars().map(|c| format!("?{c}")).collect::<String>()
                        )),
                        View::Raw,
                    )
                }
                "short" => ("^q$".into(), "q".into(), Some("q".into()), View::Raw),
                _ => {
                    let view = [View::Raw, View::Lower, View::Decoded, View::WinPath][id % 4];
                    let value = format!("k{marker}7");
                    let value = match view {
                        View::Lower => value.to_ascii_uppercase(),
                        View::Decoded => format!("%6b{marker}7"),
                        View::WinPath => value.to_ascii_uppercase(),
                        _ => value,
                    };
                    (format!("(?:k{marker}|j{marker})[0-9]"), value, None, view)
                }
            }
        };
        if variant != "short" || id == 0 {
            templates.push(value);
        }
        elastic.push(wildcard);
        items.push(SweepItem {
            id: id as i64,
            regex,
            view,
        });
    }
    templates.extend(near_misses);
    let (benign, benign_suffix) = if let Some(family) = family {
        family.background()
    } else if variant == "common" {
        ("https://common/not-a-bit/path/".to_owned(), "")
    } else {
        ("https://benign.example/path/".to_owned(), "")
    };
    let matchers: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(n, i)| {
            if family.is_some() {
                Matcher::wildcard(elastic[n].as_deref().ok_or("missing wildcard")?, i.view)
                    .map_err(|e| e.to_string())
            } else {
                Matcher::regex(&i.regex, i.view, false).map_err(|e| e.to_string())
            }
        })
        .collect::<Result<_, _>>()?;
    for matcher in &matchers {
        if matcher.is_match(format!("{benign}{}{benign_suffix}", rows - 1).as_bytes()) {
            return Err("benign template matches".into());
        }
    }
    let hits: Vec<Vec<Value>> = templates
        .iter()
        .map(|v| {
            items
                .iter()
                .zip(&matchers)
                .filter(|(_, m)| m.is_match(v.as_bytes()))
                .map(|(i, _)| json!([i.id, i.view.name()]))
                .collect()
        })
        .collect();
    let cols = ViewColumns::new("v").map_err(|e| e.to_string())?;
    let materialized = ViewColumns::conventional("v").map_err(|e| e.to_string())?;
    let plan = sweep_queries_with_options(
        "bench",
        &["id"],
        &cols,
        &items,
        None,
        SweepOptions::default(),
    )
    .map_err(|e| e.to_string())?;
    let indexed = sweep_queries_with_options(
        "bench",
        &["id"],
        &materialized,
        &items,
        None,
        SweepOptions::default(),
    )
    .map_err(|e| e.to_string())?;
    let views: std::collections::BTreeSet<_> = items.iter().map(|i| i.view).collect();
    let views: Vec<_> = views.into_iter().collect();
    let patterns: Vec<_> = items.iter().enumerate().map(|(n,i)| {
        let mut p = json!({"id":i.id,"view":i.view.name(),"regex":i.regex,"elastic_wildcard":elastic[n],"needles":watchlist_needles(&i.regex,i.view).unwrap(),"required_grams":required_grams(&matchers[n].plan)});
        if let Some(shape) = shapes[n] { p["wildcard_shape"] = json!(shape); }
        p
    }).collect();
    let pattern_bytes = serde_json::to_vec(&patterns).map_err(|e| e.to_string())?;
    let wildcard_row_predicates = if family.is_some() {
        elastic
            .iter()
            .map(|p| {
                predicate(
                    "v",
                    Kind::Wildcard,
                    p.as_deref().ok_or("missing wildcard")?,
                    View::Raw,
                )
                .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, String>>()?
    } else {
        Vec::new()
    };
    // Synthetic admission metadata. Never infer ECS source types from names or values.
    let search_target = match variant {
        "domains" | "urls" | "paths" => json!({
            "kind":"observable", "observable_type":match variant {
                "domains" => "domain", "urls" => "url", _ => "path"
            }, "wildcard_enabled":true, "policy":"comparison-fixture-v1", "synthetic":true
        }),
        _ => json!({"kind":"field", "field":"v", "data_type":"wildcard", "synthetic":true}),
    };
    Ok(
        json!({"schema":"wildcard-comparison-v2","variant":variant,"rows":rows,"indicators":count,
            "stride":if variant=="short" {10000} else {100}, "templates":templates,"template_hits":hits,"benign_prefix":benign,
            "benign_suffix":benign_suffix,
            "search_target":search_target,
            "query_contract":if family.is_some() {"case_sensitive_wildcard"} else {"fixture_regex"},
            "observable_family":if family.is_some() {Some(variant)} else {None},
            "wildcard_row_predicates":wildcard_row_predicates,
            "patterns":patterns,"pattern_sha256":format!("{:x}",Sha256::digest(&pattern_bytes)),
            "elastic_supported":variant!="mixed","elastic_exclusion":if variant=="mixed" {Some("mixed views and RE2 alternation are native/Rust qualification only")} else {None},
            "queries":plan.queries,"indexed_queries":indexed.queries,"index_ddl":view_columns_ddl("bench","v",&views).map_err(|e|e.to_string())?,
            "diagnostics":{"source_queries":plan.source_scan_count,"estimated_ast":plan.estimated_ast,"exact_only_items":plan.exact_only_items},
            "watchlist":{"ddl":watchlist_table_ddl("watch").unwrap(),"inserts":items.iter().map(|i|watchlist_insert_sql("watch",i).unwrap()).collect::<Vec<_>>(),
                "mv":watchlist_mv("ingest_mv","ingest_src","ingest_hits","watch",&["id"],&cols,&views).unwrap()}
        }),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observable_fixtures_keep_wildcards_and_reject_realistic_near_misses() {
        for family in ["domains", "urls", "paths", "commands"] {
            let spec = build(family, 12, 10000).unwrap();
            assert_eq!(spec["query_contract"], "case_sensitive_wildcard");
            assert_eq!(spec["elastic_supported"], true);
            let patterns = spec["patterns"].as_array().unwrap();
            let templates = spec["templates"].as_array().unwrap();
            assert_eq!(templates.len(), 24);
            for (i, pattern) in patterns.iter().enumerate() {
                let wildcard = pattern["elastic_wildcard"].as_str().unwrap();
                assert!(wildcard.contains('*') || wildcard.contains('?'));
                let matcher = Matcher::wildcard(wildcard, View::Raw).unwrap();
                assert!(matcher.is_match(templates[i].as_str().unwrap().as_bytes()));
                assert!(!matcher.is_match(templates[i + 12].as_str().unwrap().as_bytes()));
                assert_eq!(pattern["view"], "raw");
            }
        }
    }
    #[test]
    fn exact_hash_observables_are_outside_wildcard_workloads() {
        assert!(build("hashes", 12, 10000).is_err());
    }
    #[test]
    fn all_distributions_cover_every_indicator_and_keep_unsupported_elastic_explicit() {
        for variant in ["shared", "unrelated", "common", "short", "mixed"] {
            let spec = build(variant, 100, 10000).unwrap();
            let hits = spec["template_hits"].as_array().unwrap();
            let ids: std::collections::BTreeSet<_> = hits
                .iter()
                .flat_map(|x| x.as_array().unwrap())
                .map(|x| x[0].as_u64().unwrap())
                .collect();
            assert_eq!(ids, (0..100).collect(), "{variant}");
            assert!(!spec["queries"].as_array().unwrap().is_empty());
            assert_eq!(spec["elastic_supported"], variant != "mixed");
        }
    }
    #[test]
    fn fixture_bounds_reject_excessive_or_empty_work() {
        for count in [0, 10001] {
            assert!(build("shared", count, 10000).is_err());
        }
        for rows in [0, 1000001] {
            assert!(build("shared", 100, rows).is_err());
        }
        assert!(build("unknown", 100, 10000).is_err());
    }
}
