// SPDX-License-Identifier: Apache-2.0
use arrow_array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use std::{
    fs,
    process::{Command, Output},
    sync::Arc,
};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wc-lake"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn cli_roundtrip_binding_corruption_and_no_clobber() {
    let dir = std::env::temp_dir().join(format!("wc-identity-cli-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let input = dir.join("data.parquet");
    let output = dir.join("identities.puffin");
    let schema = Arc::new(Schema::new(vec![
        Field::new("message", DataType::Utf8, false),
        Field::new("event_id", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec!["powershell", "ordinary"])),
            Arc::new(StringArray::from(vec!["id'\\\0", "other"])),
        ],
    )
    .unwrap();
    let mut writer = ArrowWriter::try_new(fs::File::create(&input).unwrap(), schema, None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    let binding = [
        "--table-uuid",
        "00000000-0000-4000-8000-000000000001",
        "--column",
        "message",
        "--field-id",
        "1",
        "--guard",
        "event_id",
        "--guard-field-id",
        "2",
    ];
    let mapping = format!("{}=lake/file", input.display());
    let mut args = vec![
        "index-identities",
        "--out",
        output.to_str().unwrap(),
        "--snapshot-id",
        "123",
        "--sequence-number",
        "1",
        &mapping,
    ];
    args.extend(binding);
    let first = cli(&args);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let original = fs::read(&output).unwrap();
    assert!(!cli(&args).status.success());
    assert_eq!(fs::read(&output).unwrap(), original);
    let mut args = vec![
        "plan-identities",
        output.to_str().unwrap(),
        "--kind",
        "wildcard",
        "--view",
        "raw",
        "--pattern",
        "*powershell*",
        "--guard-sql",
        "on",
    ];
    args.extend(binding);
    let plan = cli(&args);
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(plan["files"][0]["values"], serde_json::json!(["id'\\\0"]));
    assert_eq!(
        plan["files"][0]["guard_sql"],
        "event_id IN ('id\\'\\\\\\x00')"
    );
    args.extend(["--column", "other"]);
    assert!(!cli(&args).status.success());
    args.truncate(args.len() - 2);
    args.extend(["--typo", "on"]);
    assert!(!cli(&args).status.success());
    args.truncate(args.len() - 2);
    fs::write(&output, &original[..original.len() - 1]).unwrap();
    let corrupt = cli(&args);
    assert!(!corrupt.status.success());
    assert!(corrupt.stdout.is_empty());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sparse_refresh_skips_cached_parquet_and_cli_refuses_corrupt_or_clobbered_output() {
    use wildcard_core::View;
    use wildcard_lake::{
        DataFile,
        identity::{index_identities, refresh_identities},
    };
    use wildcard_puffin::identity::Binding;
    let dir = std::env::temp_dir().join(format!("wc-identity-refresh-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let schema = Arc::new(Schema::new(vec![
        Field::new("message", DataType::Utf8, false),
        Field::new("event_id", DataType::Utf8, true),
    ]));
    let mut files = vec![];
    for (name, guard) in [("old", Some("same'\\\0")), ("new", None)] {
        let input = dir.join(format!("{name}.parquet"));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["target"])),
                Arc::new(StringArray::from(vec![guard])),
            ],
        )
        .unwrap();
        let mut writer =
            ArrowWriter::try_new(fs::File::create(&input).unwrap(), schema.clone(), None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        files.push(DataFile {
            local: input.to_str().unwrap().into(),
            lake_path: format!("lake/{name}"),
        });
    }
    let binding = Binding {
        table_uuid: "00000000-0000-4000-8000-000000000001".into(),
        column: "message".into(),
        guard: "event_id".into(),
        column_field_id: 1,
        guard_field_id: 2,
    };
    let old = index_identities(&files[..1], binding.clone(), &View::ALL, 8192).unwrap();
    let full = index_identities(&files, binding.clone(), &View::ALL, 8192).unwrap();
    let cache = dir.join("old.puffin");
    fs::write(&cache, old.write(122, 1).unwrap()).unwrap();
    fs::remove_file(&files[0].local).unwrap();
    let refreshed =
        refresh_identities(&files, binding.clone(), &View::ALL, 8192, Some(&old)).unwrap();
    assert_eq!(refreshed.index, full);
    assert_eq!((refreshed.indexed_files, refreshed.reused_files), (1, 1));
    assert!(refreshed.index.files()[1].unrestricted());
    fs::remove_file(&files[1].local).unwrap();
    let warm = refresh_identities(
        &files,
        binding.clone(),
        &View::ALL,
        8192,
        Some(&refreshed.index),
    )
    .unwrap();
    assert_eq!(warm.index, full);
    assert_eq!((warm.indexed_files, warm.reused_files), (0, 2));
    let mut mismatch = binding.clone();
    mismatch.guard_field_id = 3;
    assert!(
        refresh_identities(&files, mismatch, &View::ALL, 8192, Some(&old))
            .unwrap_err()
            .to_string()
            .contains("reuse profile mismatch")
    );
    let warm_cache = dir.join("warm.puffin");
    fs::write(&warm_cache, full.write(123, 2).unwrap()).unwrap();
    let output = dir.join("refreshed.puffin");
    let mappings: Vec<_> = files
        .iter()
        .map(|f| format!("{}={}", f.local, f.lake_path))
        .collect();
    let mut args = vec![
        "refresh-identities",
        "--reuse-identities",
        warm_cache.to_str().unwrap(),
        "--out",
        output.to_str().unwrap(),
        "--table-uuid",
        binding.table_uuid.as_str(),
        "--column",
        "message",
        "--field-id",
        "1",
        "--guard",
        "event_id",
        "--guard-field-id",
        "2",
        "--views",
        "raw,lower,decoded,revdomain,winpath",
        "--snapshot-id",
        "124",
        "--sequence-number",
        "3",
    ];
    args.extend(mappings.iter().map(String::as_str));
    let result = cli(&args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["indexed_files"], 0);
    assert_eq!(report["reused_files"], 2);
    assert_eq!(report["paths"], serde_json::json!(["lake/old", "lake/new"]));
    assert_eq!(fs::read(&output).unwrap(), full.write(124, 3).unwrap());
    assert!(!cli(&args).status.success());
    let retained = fs::read(&output).unwrap();
    args.extend(["--typo", "x"]);
    assert!(!cli(&args).status.success());
    args.truncate(args.len() - 2);
    args.extend(["--views", "raw"]);
    assert!(!cli(&args).status.success());
    args.truncate(args.len() - 2);
    fs::remove_file(&output).unwrap();
    fs::write(&warm_cache, b"corrupt").unwrap();
    let bad = cli(&args);
    assert!(!bad.status.success() && bad.stdout.is_empty() && !output.exists());
    assert!(!retained.is_empty());
    fs::remove_dir_all(dir).unwrap();
}
