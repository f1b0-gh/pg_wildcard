// SPDX-License-Identifier: Apache-2.0
//! Print the view-column DDL and a predicate for manual inspection.
use libwildcard::antalya::{
    MergeTreeSearchLayout, ViewColumns, column_predicate, view_columns_ddl,
};
use libwildcard::core::{View, verify::Kind};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--selective-layout"] {
        println!("-- New table only; existing tables need an explicitly managed migration.");
        println!(
            "CREATE TABLE logs (url String) ENGINE = MergeTree ORDER BY tuple() SETTINGS {};",
            MergeTreeSearchLayout::Selective.settings_sql()
        );
    } else if !args.is_empty() {
        return Err("usage: print_ddl [--selective-layout]".into());
    }
    for s in view_columns_ddl("logs", "url", &View::ALL)? {
        println!("{s};");
    }
    let cols = ViewColumns::conventional("url")?;
    println!(
        "--PRED {}",
        column_predicate(&cols, Kind::Wildcard, "*/wp-admin/*.php", View::Decoded)?
    );
    Ok(())
}
