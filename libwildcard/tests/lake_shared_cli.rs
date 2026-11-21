#![cfg(feature = "cold")]
// SPDX-License-Identifier: Apache-2.0
use arrow_array::{RecordBatch, StringArray, TimestampMicrosecondArray};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
use std::{fs, process::Command, sync::Arc};

#[test]
fn shared_cli_matches_three_reference_sidecars_reuses_and_never_clobbers() {
    let dir = std::env::temp_dir().join(format!("wc-shared-cli-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let input = dir.join("data.parquet");
    let schema = Arc::new(Schema::new(vec![
        Field::new("message", DataType::Utf8, true),
        Field::new(
            "ts",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
        Field::new("event_id", DataType::Utf8, true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![
                Some("abc target"),
                Some("ordinary"),
                None,
                Some("abc tail"),
            ])),
            Arc::new(
                TimestampMicrosecondArray::from(vec![Some(100), Some(200), None, Some(300)])
                    .with_timezone("UTC"),
            ),
            Arc::new(StringArray::from(vec![
                Some("same"),
                Some("other"),
                Some("null"),
                Some("same"),
            ])),
        ],
    )
    .unwrap();
    let mut writer = ArrowWriter::try_new(
        fs::File::create(&input).unwrap(),
        schema,
        Some(
            WriterProperties::builder()
                .set_max_row_group_row_count(Some(2))
                .build(),
        ),
    )
    .unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    let map = format!("{}=s3://warehouse/data.parquet", input.display());
    let common = vec![
        "--column",
        "message",
        "--field-id",
        "7",
        "--snapshot-id",
        "42",
        "--sequence-number",
        "2",
        "--views",
        "raw,lower,decoded,revdomain,winpath",
    ];
    let group = vec![
        "--table-uuid",
        "00000000-0000-4000-8000-000000000001",
        "--guard",
        "ts",
        "--guard-field-id",
        "9",
        "--guard-type",
        "timestamp_us_utc",
    ];
    let identity = vec![
        "--table-uuid",
        "00000000-0000-4000-8000-000000000001",
        "--guard",
        "event_id",
        "--guard-field-id",
        "3",
    ];
    let call = |args: Vec<&str>| {
        Command::new(env!("CARGO_BIN_EXE_wc-lake"))
            .args(args)
            .output()
            .unwrap()
    };
    let mut old = vec![];
    for (cmd, options) in [
        ("index", vec![]),
        ("index-groups", group.clone()),
        ("index-identities", identity),
    ] {
        let path = dir.join(format!("{cmd}.puffin"));
        let mut args = vec![cmd, "--out", path.to_str().unwrap(), &map];
        args.extend(&common);
        args.extend(options);
        let r = call(args);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        old.push(path);
    }
    let out = dir.join("joint");
    let mut args = vec![
        "refresh-search",
        "--out-dir",
        out.to_str().unwrap(),
        "--identity-guard",
        "event_id",
        "--identity-guard-field-id",
        "3",
        &map,
    ];
    args.extend(&common);
    args.extend(&group);
    let r = call(args.clone());
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let original = fs::read(out.join("manifest.json")).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(report["indexed_files"], 1);
    assert_eq!(report["identity_indexed_files"], 1);
    for (name, path) in ["files.puffin", "groups.puffin", "identities.puffin"]
        .into_iter()
        .zip(&old)
    {
        assert_eq!(
            fs::read(out.join(name)).unwrap(),
            fs::read(path).unwrap(),
            "{name}"
        );
    }
    assert!(!call(args.clone()).status.success());
    assert_eq!(fs::read(out.join("manifest.json")).unwrap(), original);
    let fresh = dir.join("bad");
    args[2] = fresh.to_str().unwrap();
    args.extend(["--views", "raw"]);
    assert!(!call(args.clone()).status.success());
    assert!(!fresh.exists());
    args.truncate(args.len() - 2);
    args.extend(["--unknown", "value"]);
    assert!(!call(args.clone()).status.success());
    assert!(!fresh.exists());
    args.truncate(args.len() - 2);
    fs::remove_file(&input).unwrap();
    let reuse_groups = out.join("groups.puffin");
    let reuse_ids = out.join("identities.puffin");
    args.extend(["--reuse-groups", reuse_groups.to_str().unwrap()]);
    assert!(!call(args.clone()).status.success());
    assert!(!fresh.exists());
    args.extend(["--reuse-identities", reuse_ids.to_str().unwrap()]);
    let base = args.clone();
    let corrupt = dir.join("corrupt.puffin");
    fs::write(&corrupt, b"PFA1 broken").unwrap();
    let incompatible = dir.join("other-snapshot.puffin");
    let idx =
        libwildcard::puffin::identity::IdentityIndex::read(&fs::read(&reuse_ids).unwrap()).unwrap();
    fs::write(&incompatible, idx.write(43, 2).unwrap()).unwrap();
    for bad in [&corrupt, &incompatible] {
        let mut rejected = base.clone();
        *rejected.last_mut().unwrap() = bad.to_str().unwrap();
        let r = call(rejected);
        assert!(!r.status.success());
        assert!(!fresh.exists());
    }
    let r = call(args);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(fresh.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(report["indexed_files"], 0);
    assert_eq!(report["reused_files"], 1);
    for name in ["files.puffin", "groups.puffin", "identities.puffin"] {
        assert_eq!(
            fs::read(out.join(name)).unwrap(),
            fs::read(fresh.join(name)).unwrap()
        );
    }
    fs::remove_dir_all(dir).unwrap();
}
