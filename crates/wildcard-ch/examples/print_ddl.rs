// SPDX-License-Identifier: Apache-2.0
//! Print the view-column DDL and a predicate for manual inspection.
use wildcard_ch::{ViewColumns, column_predicate, view_columns_ddl};
use wildcard_core::{View, verify::Kind};
fn main() {
    for s in view_columns_ddl("logs", "url", &View::ALL).unwrap() {
        println!("{s};");
    }
    let cols = ViewColumns::conventional("url").unwrap();
    println!(
        "--PRED {}",
        column_predicate(&cols, Kind::Wildcard, "*/wp-admin/*.php", View::Decoded).unwrap()
    );
}
