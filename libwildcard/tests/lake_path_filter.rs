#![cfg(feature = "hot")]
// SPDX-License-Identifier: Apache-2.0

use libwildcard::antalya::lake_path_filter;

fn paths(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

#[test]
fn excludes_only_known_impossible_files_including_with_no_candidates() {
    let indexed = paths(&["old", "candidate", "unindexed", "old"]);
    assert_eq!(
        lake_path_filter(&indexed, &paths(&["candidate", "unindexed"])),
        Some("_path NOT IN ('old')".into())
    );
    assert_eq!(
        lake_path_filter(&paths(&["old"]), &[]),
        Some("_path NOT IN ('old')".into())
    );
    assert_eq!(lake_path_filter(&indexed, &indexed), None);
    assert_eq!(lake_path_filter(&[], &[]), None);
}

#[test]
fn escapes_paths_before_applying_the_emitted_sql_budget() {
    assert_eq!(
        lake_path_filter(&paths(&["a'b\\c\n"]), &[]),
        Some("_path NOT IN ('a\\'b\\\\c\\n')".into())
    );
    // A compact raw path can expand past the guard budget when quoted.
    assert_eq!(lake_path_filter(&["\0".repeat(20_000)], &[]), None);
    assert_eq!(lake_path_filter(&["x".repeat(65_536)], &[]), None);
    let many: Vec<String> = (0..10_000).map(|i| format!("path-{i}")).collect();
    assert_eq!(lake_path_filter(&many, &[]), None);
    // Large candidate paths do not need a SQL literal at all.
    let large = "x".repeat(100_000);
    assert_eq!(
        lake_path_filter(std::slice::from_ref(&large), std::slice::from_ref(&large)),
        None
    );
}

#[test]
fn exact_boundary_keeps_a_complete_guard_and_overflow_widens() {
    let guard = lake_path_filter(&["x".repeat(65_519)], &[]).unwrap();
    assert_eq!(guard.len(), 65_536);
    assert!(guard.ends_with("')"));
    assert_eq!(lake_path_filter(&["x".repeat(65_520)], &[]), None);
    assert_eq!(
        lake_path_filter(&["x".repeat(65_519), "z".into()], &[]),
        None
    );
}
