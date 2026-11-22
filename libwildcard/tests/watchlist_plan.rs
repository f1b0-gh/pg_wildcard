// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "hot")]

use libwildcard::antalya::{
    ViewColumns, WatchlistFallback, WatchlistStrategy, watchlist_mv, watchlist_plan,
};
use libwildcard::core::View;

#[test]
fn default_watchlist_plan_preserves_existing_sql() {
    let cols = ViewColumns::new("v").unwrap();
    let plan = watchlist_plan(
        "mv",
        "logs",
        "hits",
        "watch",
        &["id"],
        &cols,
        &View::ALL,
        WatchlistStrategy::default(),
    )
    .unwrap();
    assert_eq!(plan.strategy, WatchlistStrategy::StagedNeedles);
    assert_eq!(plan.fallback, None);
    assert_eq!(plan.max_source_keys, None);
    assert_eq!(
        plan.sql,
        watchlist_mv("mv", "logs", "hits", "watch", &["id"], &cols, &View::ALL).unwrap()
    );
}

#[test]
fn requested_raw_rule_join_reports_its_extraction_bound() {
    let plan = watchlist_plan(
        "mv",
        "logs",
        "hits",
        "watch",
        &["id"],
        &ViewColumns::new("v").unwrap(),
        &[View::Raw],
        WatchlistStrategy::RuleAnchorJoin,
    )
    .unwrap();
    assert_eq!(plan.strategy, WatchlistStrategy::RuleAnchorJoin);
    assert_eq!(plan.fallback, None);
    assert_eq!(plan.max_value_bytes, Some(256));
    assert_eq!(plan.max_source_keys, Some(255));
}

#[test]
fn mixed_views_keep_the_existing_exact_strategy() {
    let cols = ViewColumns::conventional("v").unwrap();
    let views = [View::Raw, View::Lower, View::WinPath];
    let plan = watchlist_plan(
        "mv",
        "logs",
        "hits",
        "watch",
        &["id"],
        &cols,
        &views,
        WatchlistStrategy::RuleAnchorJoin,
    )
    .unwrap();
    assert_eq!(plan.strategy, WatchlistStrategy::StagedNeedles);
    assert_eq!(
        plan.fallback,
        Some(WatchlistFallback::RequiresSingleRawView)
    );
    assert_eq!(
        plan.sql,
        watchlist_mv("mv", "logs", "hits", "watch", &["id"], &cols, &views).unwrap()
    );
}

#[test]
fn occupied_internal_names_fall_back_without_changing_projection() {
    let column = (0..16).map(|i| format!("_wc{i}_")).collect::<String>();
    let cols = ViewColumns::new("v").unwrap();
    let plan = watchlist_plan(
        "mv",
        "logs",
        "hits",
        "watch",
        &[&column],
        &cols,
        &[View::Raw],
        WatchlistStrategy::RuleAnchorJoin,
    )
    .unwrap();
    assert_eq!(plan.strategy, WatchlistStrategy::StagedNeedles);
    assert_eq!(
        plan.fallback,
        Some(WatchlistFallback::InternalAliasConflict)
    );
    assert_eq!(
        plan.sql,
        watchlist_mv(
            "mv",
            "logs",
            "hits",
            "watch",
            &[&column],
            &cols,
            &[View::Raw]
        )
        .unwrap()
    );
}

#[test]
fn invalid_views_and_forged_column_identifiers_are_rejected() {
    let cols = ViewColumns::new("v").unwrap();
    for views in [&[][..], &[View::Raw, View::Raw][..]] {
        assert!(
            watchlist_plan(
                "mv",
                "logs",
                "hits",
                "watch",
                &[],
                &cols,
                views,
                WatchlistStrategy::RuleAnchorJoin
            )
            .is_err()
        );
    }
    for strategy in [
        WatchlistStrategy::StagedNeedles,
        WatchlistStrategy::RuleAnchorJoin,
    ] {
        assert!(
            watchlist_plan(
                "mv",
                "logs; DROP TABLE watch",
                "hits",
                "watch",
                &[],
                &cols,
                &[View::Raw],
                strategy
            )
            .is_err()
        );
        assert!(
            watchlist_plan(
                "mv",
                "logs",
                "hits",
                "watch",
                &["id); SELECT 1"],
                &cols,
                &[View::Raw],
                strategy
            )
            .is_err()
        );
        let mut forged = cols.clone();
        forged.raw = "v); SELECT 1".into();
        assert!(
            watchlist_plan(
                "mv",
                "logs",
                "hits",
                "watch",
                &[],
                &forged,
                &[View::Raw],
                strategy
            )
            .is_err()
        );
        forged = cols.clone();
        forged.materialized.insert(View::Raw, "v); SELECT 1".into());
        assert!(
            watchlist_plan(
                "mv",
                "logs",
                "hits",
                "watch",
                &[],
                &forged,
                &[View::Raw],
                strategy
            )
            .is_err()
        );
    }
}
