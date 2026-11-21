use libwildcard::core::{Matcher, View};

#[test]
fn core_is_available_without_a_database() -> Result<(), Box<dyn std::error::Error>> {
    let matcher = Matcher::wildcard("*/wp-admin/*.php", View::Decoded)?;
    assert!(matcher.is_match(b"GET /%77p-ADMIN/a.PHP"));
    assert!(!matcher.is_match(b"GET /static/a.js"));
    Ok(())
}

#[cfg(feature = "hot")]
#[test]
fn hot_uses_safe_sql_and_shared_semantics() -> Result<(), Box<dyn std::error::Error>> {
    use libwildcard::{antalya, core::Kind};
    assert_eq!(
        antalya::predicate("url", Kind::Wildcard, "*%_?", View::Raw)?,
        "like(url, '%\\\\%\\\\__')"
    );
    assert!(antalya::ident("logs.http").is_ok());
    assert!(antalya::ident("url; DROP TABLE logs").is_err());
    let known = vec!["s3://bucket/old.parquet".to_string()];
    assert!(
        antalya::lake_query_with_coverage("T", "*", "1", &known, &["foreign".into()], None)
            .is_err()
    );
    let sql = antalya::lake_query_with_coverage("T", "*", "1", &known, &[], None)?;
    assert!(sql.contains("_path NOT IN ('s3://bucket/old.parquet')"));
    assert!(!antalya::lake_query_with_coverage("T", "*", "1", &[], &[], None)?.contains("_path"));
    Ok(())
}

#[cfg(feature = "puffin")]
#[test]
fn cold_index_roundtrip_keeps_unindexed_candidates() -> Result<(), Box<dyn std::error::Error>> {
    use libwildcard::puffin::{self, IndexInfo, ngram::NgramIndexBuilder};
    let mut builder = NgramIndexBuilder::new(&[View::Raw], 32);
    let file = builder.add_file("s3://bucket/data/a.parquet");
    builder.add_value(file, b"evil.test");
    let oversized = builder.add_file("s3://bucket/data/unindexed.parquet");
    builder.add_value(oversized, &[b'x'; 33]);
    let index = builder.finish();
    let info = IndexInfo {
        column: "url".into(),
        field_id: 1,
        snapshot_id: 42,
        sequence_number: 7,
        max_value_bytes: 32,
    };
    let encoded = puffin::write_index(&index, &info)?;
    let decoded = puffin::read_indexes(&encoded)?;
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].0.snapshot_id, 42);
    assert_eq!(decoded[0].1.files[0].path, "s3://bucket/data/a.parquet");
    let selected =
        decoded[0]
            .1
            .candidates(libwildcard::core::Kind::Wildcard, "*evil.test*", View::Raw)?;
    assert_eq!(selected.files.len(), 2);
    let absent = decoded[0].1.candidates(
        libwildcard::core::Kind::Wildcard,
        "*absent.test*",
        View::Raw,
    )?;
    assert_eq!(absent.files.len(), 1);
    assert_eq!(absent.files[0].path, "s3://bucket/data/unindexed.parquet");
    assert!(puffin::read_indexes(b"invalid").is_err());
    Ok(())
}
