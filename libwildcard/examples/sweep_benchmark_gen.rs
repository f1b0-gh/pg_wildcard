// SPDX-License-Identifier: Apache-2.0
//! Generate the three sweep strategies for the scale benchmark.
use libwildcard::antalya::{
    SweepItem, ViewColumns, quote, sweep_queries, watchlist_insert_sql, watchlist_mv,
    watchlist_table_ddl,
};
use libwildcard::core::View;

fn main() {
    let items: Vec<_> = (0..1000)
        .map(|id| SweepItem {
            id,
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
    let proposed = sweep_queries(
        "bench",
        &["id"],
        &ViewColumns::new("v").unwrap(),
        &items,
        None,
        200_000,
    )
    .unwrap();
    let watch_baseline = |indices: &str| {
        format!(
            "CREATE MATERIALIZED VIEW ingest_mv TO ingest_hits AS WITH (SELECT (groupArray(id), groupArray(regex)) FROM (SELECT id, regex FROM watch FINAL WHERE active = 1 AND view = 'raw' ORDER BY id)) AS w, tupleElement(w, 1) AS ids, tupleElement(w, 2) AS pats SELECT id, arrayElement(ids, i) AS indicator_id, 'raw' AS view FROM ingest_src ARRAY JOIN arrayFilter(i -> match(v, arrayElement(pats, i)), {indices}) AS i"
        )
    };
    let watch_mvs = serde_json::json!({
        "original_hyperscan": watch_baseline("multiMatchAllIndices(v, pats)"),
        "exhaustive_exact": watch_baseline("arrayEnumerate(pats)"),
        "dynamic_needles": watchlist_mv("ingest_mv", "ingest_src", "ingest_hits", "watch", &["id"], &ViewColumns::new("v").unwrap(), &[View::Raw]).unwrap(),
    });
    println!(
        "{}",
        serde_json::json!({
            "rows": 1_000_000, "indicators": items.len(),
            "watchlist": {
                "ddl": watchlist_table_ddl("watch").unwrap(),
                "inserts": items.iter().map(|i| watchlist_insert_sql("watch", i).unwrap()).collect::<Vec<_>>(),
                "mvs": watch_mvs,
            },
            "queries": {
                "original_hyperscan": [baseline("multiMatchAllIndices(v, pats)")],
                "exhaustive_exact": [baseline("arrayEnumerate(pats)")],
                "constant_candidates": proposed,
            }
        })
    );
}
