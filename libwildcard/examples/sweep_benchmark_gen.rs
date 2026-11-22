// SPDX-License-Identifier: Apache-2.0
//! Reproducible exact, fixed-64 and AST-budgeted sweep/watchlist strategies.
use libwildcard::antalya::{
    GramFileCounts, SweepItem, SweepOptions, ViewColumns, candidate_predicate, predicate, quote,
    sweep_queries_with_options, watchlist_insert_sql, watchlist_insert_sql_with_frequencies,
    watchlist_mv, watchlist_needles, watchlist_needles_with_frequencies, watchlist_table_ddl,
};
use libwildcard::core::{View, verify::Kind};

// Frozen pre-budgeting 64-item tuple shape: never call the evolving sweep helper
// here, otherwise a production optimization silently changes its own baseline.
fn legacy_dense_sweeps(items: &[SweepItem], cols: &ViewColumns) -> Vec<String> {
    items.chunks(64).map(|chunk| {
        let expressions: Vec<_> = chunk.iter().map(|item| {
            let value=cols.expr(item.view); let kind=Kind::Regex {case_insensitive:false};
            let exact=predicate(&value,kind,&item.regex,item.view).unwrap();
            let condition=candidate_predicate(&value,kind,&item.regex,item.view).unwrap()
                .map_or_else(||exact.clone(),|candidate|format!("({candidate}) AND ({exact})"));
            format!("(toInt64({}), '{}', ({condition}))",item.id,item.view.name())
        }).collect();
        let sql=format!("/* wildcard sweep: strategy=constant_candidates source_scans=1 source_scan_count=1                    */\nSELECT id, tupleElement(wc_hit, 1) AS indicator_id, tupleElement(wc_hit, 2) AS view\nFROM bench\nARRAY JOIN arrayMap(x -> (tupleElement(x, 1), tupleElement(x, 2)), arrayFilter(x -> tupleElement(x, 3), [{}])) AS wc_hit",expressions.join(", "));
        assert!(sql.len()<=200_000);sql
    }).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value = |name: &str| {
        args.windows(2)
            .find(|a| a[0] == name)
            .map(|a| a[1].as_str())
    };
    let rows: usize = value("--rows").unwrap_or("1000000").parse().unwrap();
    let count: usize = value("--indicators").unwrap_or("1000").parse().unwrap();
    assert!(rows > 0 && count > 0 && count <= 10_000);
    let frequencies: Option<GramFileCounts> = value("--frequencies").map(|path| {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .skip(1)
            .map(|line| {
                let (gram, count) = line.split_once(',').unwrap();
                (gram.parse().unwrap(), count.parse().unwrap())
            })
            .collect()
    });
    let items: Vec<_> = (0..count)
        .map(|id| SweepItem {
            id: id as i64,
            view: View::Raw,
            regex: format!("evil{id:04}\\.example"),
        })
        .collect();
    let pats = items
        .iter()
        .map(|i| quote(&i.regex))
        .collect::<Vec<_>>()
        .join(", ");
    let ids = items
        .iter()
        .map(|i| i.id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let baseline = |indices: &str| {
        format!(
            "WITH [{pats}] AS pats, [{ids}] AS ids SELECT id, arrayElement(ids, i) AS indicator_id, 'raw' AS view FROM bench ARRAY JOIN arrayFilter(i -> match(v, arrayElement(pats, i)), {indices}) AS i"
        )
    };
    let cols = ViewColumns::new("v").unwrap();
    let plan = sweep_queries_with_options(
        "bench",
        &["id"],
        &cols,
        &items,
        None,
        SweepOptions::default(),
    )
    .unwrap();
    let fixed64 = legacy_dense_sweeps(&items, &cols);
    let watch_baseline = |indices: &str| {
        format!(
            "CREATE MATERIALIZED VIEW ingest_mv TO ingest_hits AS WITH (SELECT (groupArray(id), groupArray(regex)) FROM (SELECT id, regex FROM watch FINAL WHERE active = 1 AND view = 'raw' ORDER BY id)) AS w, tupleElement(w, 1) AS ids, tupleElement(w, 2) AS pats SELECT id, arrayElement(ids, i) AS indicator_id, 'raw' AS view FROM ingest_src ARRAY JOIN arrayFilter(i -> match(v, arrayElement(pats, i)), {indices}) AS i"
        )
    };
    let mv = watchlist_mv(
        "ingest_mv",
        "ingest_src",
        "ingest_hits",
        "watch",
        &["id"],
        &cols,
        &[View::Raw],
    )
    .unwrap();
    // Frozen scalar-AND MV from the measured baseline, independent of the current helper.
    let old_and_mv = r#"CREATE MATERIALIZED VIEW IF NOT EXISTS ingest_mv TO ingest_hits AS
WITH (SELECT (groupArray(id), groupArray(regex), groupArray(if(needles_regex = regex AND needles_view = view, arrayElement(needles, 1), '')), groupArray(if(needles_regex = regex AND needles_view = view, arrayElement(needles, 2), '')), groupArray(if(needles_regex = regex AND needles_view = view, arrayElement(needles, 3), ''))) FROM (SELECT id, regex, view, needles, needles_regex, needles_view FROM watch FINAL WHERE active = 1 AND view = 'raw' ORDER BY id)) AS wc_w_raw
SELECT id, tupleElement(wc_hit, 1) AS indicator_id, tupleElement(wc_hit, 2) AS view
FROM ingest_src
ARRAY JOIN arrayConcat(arrayMap(i -> (arrayElement(tupleElement(wc_w_raw, 1), i), 'raw'), arrayFilter(i -> position(v, arrayElement(tupleElement(wc_w_raw, 3), i)) > 0 AND position(v, arrayElement(tupleElement(wc_w_raw, 4), i)) > 0 AND position(v, arrayElement(tupleElement(wc_w_raw, 5), i)) > 0 AND match(v, arrayElement(tupleElement(wc_w_raw, 2), i)), arrayEnumerate(tupleElement(wc_w_raw, 2))))) AS wc_hit"#;
    // Historical lexical selection for this literal-only corpus is always '.ex'.
    let legacy_inserts: Vec<_> = items.iter().map(|i| format!(
        "INSERT INTO watch (id, view, regex, needles, needles_regex, needles_view) VALUES ({}, 'raw', {}, ['.ex'], {}, 'raw')", i.id, quote(&i.regex), quote(&i.regex)
    )).collect();
    let legacy_mv = "CREATE MATERIALIZED VIEW ingest_mv TO ingest_hits AS WITH (SELECT (groupArray(id), groupArray(regex), groupArray(if(needles_regex=regex AND needles_view=view,arrayElement(needles,1),''))) FROM (SELECT id,regex,view,needles,needles_regex,needles_view FROM watch FINAL WHERE active=1 AND view='raw' ORDER BY id)) AS w SELECT id,arrayElement(tupleElement(w,1),i) AS indicator_id,'raw' AS view FROM ingest_src ARRAY JOIN arrayFilter(i -> position(v,arrayElement(tupleElement(w,3),i))>0 AND match(v,arrayElement(tupleElement(w,2),i)),arrayEnumerate(tupleElement(w,2))) AS i";
    println!(
        "{}",
        serde_json::json!({
            "rows":rows,"indicators":count,"corpus":"url-evil-every-100-v1",
            "frequencies_supplied":frequencies.is_some(),
            "diagnostics":{"strategy":"constant_candidates","source_scan_count":plan.source_scan_count,"estimated_ast":plan.estimated_ast,"exact_only_items":plan.exact_only_items},
            "needles":{
                "legacy_lexical":items.iter().map(|_|vec![".ex"]).collect::<Vec<_>>(),
                "ranked_needles":items.iter().map(|i|watchlist_needles(&i.regex,i.view).unwrap()).collect::<Vec<_>>(),
                "frequency_needles":items.iter().map(|i|watchlist_needles_with_frequencies(&i.regex,i.view,frequencies.as_ref()).unwrap()).collect::<Vec<_>>()
            },
            "watchlist":{"ddl":watchlist_table_ddl("watch").unwrap(),
                "inserts":{
                    "legacy_lexical":legacy_inserts,
                    "ranked_needles":items.iter().map(|i|watchlist_insert_sql("watch",i).unwrap()).collect::<Vec<_>>(),
                    "frequency_needles":items.iter().map(|i|watchlist_insert_sql_with_frequencies("watch",i,frequencies.as_ref()).unwrap()).collect::<Vec<_>>(),
                    "legacy_ranked_and":items.iter().map(|i|watchlist_insert_sql("watch",i).unwrap()).collect::<Vec<_>>(),
                    "legacy_frequency_and":items.iter().map(|i|watchlist_insert_sql_with_frequencies("watch",i,frequencies.as_ref()).unwrap()).collect::<Vec<_>>()},
                "mvs":{
                    "original_hyperscan":watch_baseline("multiMatchAllIndices(v,pats)"),
                    "exhaustive_exact":watch_baseline("arrayEnumerate(pats)"),
                    "legacy_lexical":legacy_mv,"ranked_needles":mv,"frequency_needles":mv,
                    "legacy_ranked_and":old_and_mv,"legacy_frequency_and":old_and_mv}},
            "queries":{"original_hyperscan":[baseline("multiMatchAllIndices(v,pats)")],
                "exhaustive_exact":[baseline("arrayEnumerate(pats)")],"fixed64_candidates":fixed64,"constant_candidates":plan.queries}
        })
    );
}
