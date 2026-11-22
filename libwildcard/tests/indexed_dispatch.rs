// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "hot")]
use libwildcard::antalya::{
    ByteDispatchFallback, ByteDispatchGate, ChError, SweepItem, SweepOptions, SweepStrategy,
    ViewColumns, byte_gram_index_ddl, sweep_byte_dispatch,
};
use libwildcard::core::View;

fn rules(count: usize) -> Vec<SweepItem> {
    (0..count)
        .map(|i| SweepItem {
            id: i as i64,
            view: View::Raw,
            regex: format!("evil{i:04}\\.example"),
        })
        .collect()
}

#[test]
fn thousand_rules_dispatch_in_one_bounded_query() {
    let plan = sweep_byte_dispatch(
        "events",
        &["id"],
        &ViewColumns::new("v").unwrap(),
        &rules(1000),
        None,
        SweepOptions::default(),
        None,
    )
    .unwrap();
    assert_eq!(plan.sweep.strategy, SweepStrategy::ByteDispatch);
    assert_eq!(plan.sweep.source_scan_count, 1);
    assert!(plan.fallback.is_none());
    assert!(plan.index_gate);
    assert_eq!(plan.gate_strategy, ByteDispatchGate::RequiredFragments);
    assert!((1..=255).contains(&plan.dispatch_groups));
    assert!(plan.sweep.queries[0].contains("multiSearchAllPositions("));
    assert!(plan.sweep.queries[0].contains("match("));
    assert!(plan.sweep.queries[0].contains("_wildcard_overflow_"));
    assert!(plan.sweep.queries[0].len() <= 200_000);
    assert!(plan.sweep.estimated_ast[0] <= 45_000);
}

#[test]
fn unselective_and_other_views_fall_back_explicitly() {
    for items in [
        vec![
            SweepItem {
                id: 0,
                view: View::Raw,
                regex: ".*".into()
            };
            100
        ],
        vec![
            SweepItem {
                id: 0,
                view: View::Lower,
                regex: "abc".into()
            };
            100
        ],
        rules(2001),
    ] {
        let plan = sweep_byte_dispatch(
            "events",
            &["id"],
            &ViewColumns::new("v").unwrap(),
            &items,
            None,
            SweepOptions::default(),
            None,
        )
        .unwrap();
        assert_eq!(plan.sweep.strategy, SweepStrategy::ConstantCandidates);
        assert!(plan.fallback.is_some());
        assert!(!plan.index_gate);
    }
}

#[test]
fn explicit_index_caps_bytes_and_retains_overflow() {
    let ddl = byte_gram_index_ddl("events", "v_byte_grams", "v").unwrap();
    assert!(ddl.contains("hex(substring(v, 1, 8192))"));
    assert!(ddl.contains("_wildcard_overflow_"));
    assert!(ddl.contains("tokenizer='array'"));
    assert!(byte_gram_index_ddl("events; DROP TABLE x", "i", "v").is_err());
}

#[test]
fn budget_widening_and_invalid_input_keep_the_exact_contract() {
    let cols = ViewColumns::new("v").unwrap();
    let items = rules(1);
    let full = sweep_byte_dispatch(
        "events",
        &["id"],
        &cols,
        &items,
        None,
        SweepOptions::default(),
        None,
    )
    .unwrap();
    let options = SweepOptions {
        max_bytes: full.sweep.queries[0].len() - 1,
        ..SweepOptions::default()
    };
    let plan = sweep_byte_dispatch("events", &["id"], &cols, &items, None, options, None).unwrap();
    assert_eq!(plan.fallback, None);
    assert_eq!(plan.gate_strategy, ByteDispatchGate::SingleFragmentBudget);
    assert!(plan.sweep.queries[0].len() <= options.max_bytes);
    assert!(plan.sweep.queries[0].contains("match("));
    let smaller = SweepOptions {
        max_bytes: plan.sweep.queries[0].len() - 1,
        ..options
    };
    let dense = sweep_byte_dispatch("events", &["id"], &cols, &items, None, smaller, None).unwrap();
    assert_eq!(dense.fallback, Some(ByteDispatchFallback::QueryBudget));
    assert_eq!(dense.gate_strategy, ByteDispatchGate::Disabled);
    assert!(
        dense
            .sweep
            .queries
            .iter()
            .all(|q| q.len() <= smaller.max_bytes)
    );
    let ast_limited = SweepOptions {
        max_ast_nodes: full.sweep.estimated_ast[0] - 1,
        ..SweepOptions::default()
    };
    let coarse =
        sweep_byte_dispatch("events", &["id"], &cols, &items, None, ast_limited, None).unwrap();
    assert_eq!(coarse.gate_strategy, ByteDispatchGate::SingleFragmentBudget);
    assert!(coarse.sweep.estimated_ast[0] <= ast_limited.max_ast_nodes);
    let invalid = [SweepItem {
        regex: "(?=x)".into(),
        ..items[0].clone()
    }];
    assert!(sweep_byte_dispatch("events", &[], &cols, &invalid, None, options, None).is_err());
    assert_eq!(
        sweep_byte_dispatch(
            "events",
            &[],
            &cols,
            &[],
            None,
            SweepOptions {
                max_bytes: 0,
                ..options
            },
            None
        ),
        Err(ChError::InvalidSweepOptions)
    );
}

#[test]
fn unsupported_rules_stay_in_dispatch_and_disable_only_the_index_gate() {
    let mut items = rules(16);
    items.push(SweepItem {
        id: i64::MAX,
        view: View::Raw,
        regex: ".*".into(),
    });
    let plan = sweep_byte_dispatch(
        "events",
        &[],
        &ViewColumns::new("v").unwrap(),
        &items,
        None,
        SweepOptions::default(),
        None,
    )
    .unwrap();
    assert!(plan.fallback.is_none());
    assert!(!plan.index_gate);
    assert_eq!(plan.sweep.exact_only_items, 1);
    assert!(plan.sweep.queries[0].contains("toUInt64(17), ''"));
}
