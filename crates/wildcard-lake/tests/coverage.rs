// SPDX-License-Identifier: Apache-2.0
use libwildcard::{
    core::View,
    puffin::{IndexInfo, ngram::NgramIndexBuilder, write_index},
};
use std::{error::Error, fs, process::Command};

#[test]
fn sql_keeps_unknown_files_even_when_the_index_has_no_matching_files() -> Result<(), Box<dyn Error>>
{
    let parent = std::path::Path::new("/tmp/libwildcard");
    fs::create_dir_all(parent)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = parent.join(format!("coverage-{}-{nonce}", std::process::id()));
    fs::create_dir(&directory)?;
    let file = directory.join("old.puffin");
    let mut builder = NgramIndexBuilder::new(&[View::Raw], 8192);
    let id = builder.add_file("s3://bucket/old.parquet");
    builder.add_value(id, b"ordinary");
    let bytes = write_index(
        &builder.finish(),
        &IndexInfo {
            column: "url".into(),
            field_id: 1,
            snapshot_id: 1,
            sequence_number: 1,
            max_value_bytes: 8192,
        },
    )?;
    fs::write(&file, bytes)?;
    for pattern in ["*evil.test*", "*ordinary*"] {
        let output = Command::new(env!("CARGO_BIN_EXE_wc-lake"))
            .arg("sql")
            .arg(&file)
            .args([
                "--table",
                "T",
                "--kind",
                "wildcard",
                "--view",
                "raw",
                "--pattern",
                pattern,
            ])
            .output()?;
        assert!(output.status.success());
        let sql = String::from_utf8(output.stdout)?;
        assert!(
            !sql.trim().is_empty(),
            "A stale index cannot prove the current table has no matches"
        );
        assert!(
            sql.contains("NOT IN") || !sql.contains("_path"),
            "Unknown current files must remain eligible: {sql}"
        );
    }
    fs::remove_dir_all(directory)?;
    Ok(())
}
