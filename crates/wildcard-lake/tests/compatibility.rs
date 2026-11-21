// SPDX-License-Identifier: Apache-2.0
//! Existing consumers must exchange values with libwildcard without adapters.
#[test]
fn legacy_names_share_public_types() -> Result<(), Box<dyn std::error::Error>> {
    let view: libwildcard::core::View = wildcard_core::View::Raw;
    let matcher: wildcard_core::Matcher = libwildcard::core::Matcher::wildcard("*evil*", view)?;
    assert!(matcher.is_match(b"evil.example"));
    let columns: libwildcard::antalya::ViewColumns = wildcard_ch::ViewColumns::new("message")?;
    let _: wildcard_ch::ViewColumns = columns;
    let mut builder = wildcard_puffin::ngram::NgramIndexBuilder::new(&[view], 32);
    let file = builder.add_file("s3://bucket/a.parquet");
    builder.add_value(file, b"evil.example");
    let index: libwildcard::puffin::ngram::NgramIndex = builder.finish();
    assert_eq!(
        index
            .candidates(wildcard_core::Kind::Wildcard, "*evil*", view)?
            .files
            .len(),
        1
    );
    let file: libwildcard::lake::DataFile = wildcard_lake::DataFile {
        local: "a.parquet".into(),
        lake_path: "s3://bucket/a.parquet".into(),
    };
    let _: wildcard_lake::DataFile = file;
    Ok(())
}
