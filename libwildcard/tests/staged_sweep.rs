// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "hot")]
use libwildcard::antalya::{
    LiteralGate, StagingBudget, SweepItem, SweepOptions, ViewColumns, staged_literal_sweep,
};
use libwildcard::core::View;

fn items(literals: &[&str]) -> Vec<SweepItem> {
    literals
        .iter()
        .enumerate()
        .map(|(i, literal)| SweepItem {
            id: i as i64,
            view: View::Raw,
            regex: regex::escape(literal),
        })
        .collect()
}

#[test]
fn staging_keeps_exact_checks_without_an_index_gate() {
    let rules: Vec<_> = (0..1000)
        .map(|i| SweepItem {
            id: i,
            view: View::Raw,
            regex: format!("evil{i:04}\\.example"),
        })
        .collect();
    let p = staged_literal_sweep(
        "events",
        "candidates",
        &["id"],
        &ViewColumns::new("v").unwrap(),
        &rules,
        Some("id % 2 = 0 OR id = 1 -- trusted source filter"),
        SweepOptions::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(p.gate, LiteralGate::Prefix("evil0".into()));
    assert!(p.candidate_select.contains("position(v,'evil0') > 0"));
    assert!(
        p.candidate_select
            .contains("\nAND (\nid % 2 = 0 OR id = 1 -- trusted source filter\n)")
    );
    assert_eq!(p.queries.len(), 4);
    for q in p.queries {
        assert!(q.contains("FROM candidates"));
        assert!(q.contains("match("));
        assert!(q.contains("multiSearchAllPositions("));
        assert!(!q.contains("ngrams("));
        assert!(!q.contains("hasAll("));
    }
}

#[test]
fn mask_is_necessary_and_sql_literals_are_escaped() {
    let rules = items(&["abc';x", "xyz];x"]);
    let p = staged_literal_sweep(
        "events",
        "candidates",
        &["id"],
        &ViewColumns::new("v").unwrap(),
        &rules,
        None,
        SweepOptions::default(),
    )
    .unwrap()
    .unwrap();
    let LiteralGate::CharacterMask(mask) = p.gate else {
        panic!("expected mask")
    };
    let re = regex::bytes::Regex::new(&mask).unwrap();
    for value in [b"abc';x".as_slice(), b"\xffxyz];x\0", b"ayc];x"] {
        assert!(re.is_match(value));
    }
    assert!(!re.is_match(b"nothing here"));
    let p = staged_literal_sweep(
        "events",
        "candidates",
        &["id"],
        &ViewColumns::new("v").unwrap(),
        &items(&["x'y000", "x'y001"]),
        None,
        SweepOptions::default(),
    )
    .unwrap()
    .unwrap();
    assert!(p.candidate_select.contains("x\\'y00"));
}

#[test]
fn unsupported_shapes_decline_without_weakening_the_fallback() {
    for regex in [
        ".*", "^abc", "(?i)abc", "a|b", "évil", "a", "", "abc\\x00", "abc\\xFF",
    ] {
        let rules = [SweepItem {
            id: 0,
            view: View::Raw,
            regex: regex.into(),
        }];
        assert!(
            staged_literal_sweep(
                "events",
                "candidates",
                &["id"],
                &ViewColumns::new("v").unwrap(),
                &rules,
                None,
                SweepOptions::default()
            )
            .unwrap()
            .is_none(),
            "{regex}"
        );
    }
    let mut rules = items(&["abc000", "abc001"]);
    rules[0].view = View::Lower;
    assert!(
        staged_literal_sweep(
            "events",
            "candidates",
            &["id"],
            &ViewColumns::new("v").unwrap(),
            &rules,
            None,
            SweepOptions::default()
        )
        .unwrap()
        .is_none()
    );
    assert!(
        staged_literal_sweep(
            "events; DROP x",
            "candidates",
            &["id"],
            &ViewColumns::new("v").unwrap(),
            &[],
            None,
            SweepOptions::default()
        )
        .is_err()
    );
    assert!(
        staged_literal_sweep(
            "events",
            "events",
            &["id"],
            &ViewColumns::new("v").unwrap(),
            &[],
            None,
            SweepOptions::default()
        )
        .is_err()
    );
}

#[test]
fn admission_refuses_dense_or_oversized_candidates_without_overflow() {
    let budget = StagingBudget {
        max_rows: 100_000,
        max_value_bytes: 8_000_000,
        max_selectivity_per_mille: 100,
    };
    assert!(budget.allows(1_000_000, 10_000, 340_000));
    assert!(budget.allows(1_000_000, 100_000, 8_000_000));
    assert!(!budget.allows(1_000_000, 1_000_000, 340_000));
    assert!(!budget.allows(1_000_000, 10_000, 8_000_001));
    assert!(!budget.allows(1, 2, 0));
    assert!(!budget.allows(0, 1, 0));
    assert!(budget.allows(0, 0, 0));
    assert!(!budget.allows(0, 0, 1));
    let huge = StagingBudget {
        max_rows: u64::MAX,
        max_value_bytes: u64::MAX,
        max_selectivity_per_mille: 1000,
    };
    assert!(huge.allows(u64::MAX, u64::MAX, u64::MAX));
    assert!(
        !StagingBudget {
            max_selectivity_per_mille: 1001,
            ..huge
        }
        .allows(1, 1, 1)
    );
}

#[test]
fn staging_admission_and_query_budgets_bound_planner_work() {
    let cols = ViewColumns::new("v").unwrap();
    let rules: Vec<_> = (0..2001)
        .map(|i| SweepItem {
            id: i,
            view: View::Raw,
            regex: format!("evil{i:04}\\.example"),
        })
        .collect();
    let plan = staged_literal_sweep(
        "events",
        "candidates",
        &["id"],
        &cols,
        &rules[..2000],
        None,
        SweepOptions::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(plan.queries.len(), 8);
    assert!(
        staged_literal_sweep(
            "events",
            "candidates",
            &["id"],
            &cols,
            &rules,
            None,
            SweepOptions::default()
        )
        .unwrap()
        .is_none()
    );
    for (literal, admitted) in [("x".repeat(64), true), ("x".repeat(65), false)] {
        let rules = items(&[&literal]);
        assert_eq!(
            staged_literal_sweep(
                "events",
                "candidates",
                &["id"],
                &cols,
                &rules,
                None,
                SweepOptions::default()
            )
            .unwrap()
            .is_some(),
            admitted
        );
    }
    let huge = [SweepItem {
        id: 0,
        view: View::Raw,
        regex: "x".repeat(100_000),
    }];
    assert!(
        staged_literal_sweep(
            "events",
            "candidates",
            &["id"],
            &cols,
            &huge,
            None,
            SweepOptions::default()
        )
        .unwrap()
        .is_none()
    );
    let rules = items(&["abc000", "abc001"]);
    for options in [
        SweepOptions {
            max_bytes: 10,
            ..SweepOptions::default()
        },
        SweepOptions {
            max_ast_nodes: 1,
            ..SweepOptions::default()
        },
        SweepOptions {
            max_bytes: 0,
            ..SweepOptions::default()
        },
    ] {
        assert!(
            staged_literal_sweep(
                "events",
                "candidates",
                &["id"],
                &cols,
                &rules,
                None,
                options
            )
            .is_err()
        );
    }
    assert!(
        staged_literal_sweep(
            "events",
            "candidates",
            &["events.id"],
            &cols,
            &rules,
            None,
            SweepOptions::default()
        )
        .unwrap()
        .is_none()
    );
    let selected = ["id", "indicator_id", "view", "id"];
    let p = staged_literal_sweep(
        "events",
        "candidates",
        &selected,
        &cols,
        &rules,
        None,
        SweepOptions::default(),
    )
    .unwrap()
    .unwrap();
    assert!(
        p.candidate_select
            .starts_with("SELECT id, indicator_id, view, v\n")
    );
    assert!(
        p.queries
            .iter()
            .all(|q| q.len() <= SweepOptions::default().max_bytes)
    );
}
