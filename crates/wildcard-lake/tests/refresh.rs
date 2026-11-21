// SPDX-License-Identifier: Apache-2.0
use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use wildcard_core::View;
use wildcard_lake::{
    DataFile, index_parquet,
    rowgroup::{index_groups, refresh_groups},
};
use wildcard_puffin::rowgroup::{Binding, GroupIndex, GuardType};

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "wc-refresh-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn binding() -> Binding {
    Binding {
        table_uuid: "00000000-0000-0000-0000-000000000001".into(),
        column: "message".into(),
        guard: "id".into(),
        column_field_id: 2,
        guard_field_id: 1,
        guard_type: GuardType::Int64,
    }
}
fn write(path: &Path, values: Vec<Option<&str>>) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, true),
        Field::new("message", DataType::Utf8, true),
    ]));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(
            (0..values.len())
                .map(|i| if i == 1 { None } else { Some(i as i64) })
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(values)),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(2))
        .build();
    let mut writer =
        ArrowWriter::try_new(fs::File::create(path).unwrap(), schema, Some(props)).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}
fn data(path: &Path, name: &str) -> DataFile {
    DataFile {
        local: path.to_str().unwrap().into(),
        lake_path: format!("s3://warehouse/{name}.parquet"),
    }
}

#[test]
fn refresh_skips_cached_inputs_and_matches_full_rebuild_after_append_and_remove() {
    let t = Temp::new();
    let a = t.0.join("a.parquet");
    let b = t.0.join("b.parquet");
    write(
        &a,
        vec![
            Some("PowerShell.exe"),
            None,
            Some("https://evil.example/%2e%2e"),
        ],
    );
    let long = "x".repeat(65);
    write(
        &b,
        vec![
            Some("C:\\Windows\\Temp\\a.exe"),
            Some(&long),
            Some("powershell appended"),
        ],
    );
    let old = index_groups(&[data(&a, "a")], binding(), &View::ALL, 64).unwrap();
    let full = index_groups(&[data(&b, "b"), data(&a, "a")], binding(), &View::ALL, 64).unwrap();
    let file_full =
        index_parquet(&[data(&b, "b"), data(&a, "a")], "message", &View::ALL, 64).unwrap();
    fs::remove_file(&a).unwrap();
    let result = refresh_groups(
        &[data(&b, "b"), data(&a, "a")],
        binding(),
        &View::ALL,
        64,
        Some(&old),
    )
    .unwrap();
    assert_eq!(result.index, full);
    assert_eq!(
        result.index.file_index().unwrap().encode(),
        file_full.encode()
    );
    assert_eq!(
        (
            result.indexed_files,
            result.reused_files,
            result.indexed_groups,
            result.reused_groups
        ),
        (1, 1, 2, 2)
    );
    fs::remove_file(&b).unwrap();
    let no_change = refresh_groups(
        &[data(&b, "b"), data(&a, "a")],
        binding(),
        &View::ALL,
        64,
        Some(&result.index),
    )
    .unwrap();
    assert_eq!((no_change.indexed_files, no_change.reused_files), (0, 2));
    assert_eq!(no_change.index, full);
    let removed = refresh_groups(
        &[data(&b, "b")],
        binding(),
        &View::ALL,
        64,
        Some(&result.index),
    )
    .unwrap();
    assert_eq!(removed.index.files().len(), 1);
    assert_eq!(removed.index.files()[0].path, "s3://warehouse/b.parquet");
    let empty = refresh_groups(&[], binding(), &View::ALL, 64, Some(&result.index)).unwrap();
    assert!(empty.index.files().is_empty());
    let replacement = t.0.join("new.parquet");
    write(&replacement, vec![Some("new powershell match")]);
    let replaced = refresh_groups(
        &[data(&replacement, "replacement")],
        binding(),
        &View::ALL,
        64,
        Some(&result.index),
    )
    .unwrap();
    assert_eq!((replaced.indexed_files, replaced.reused_files), (1, 0));
    assert_eq!(
        replaced.index,
        index_groups(
            &[data(&replacement, "replacement")],
            binding(),
            &View::ALL,
            64
        )
        .unwrap()
    );
}

#[test]
fn refresh_refuses_incompatible_profile_before_opening_data() {
    let old = index_groups(&[], binding(), &View::ALL, 64).unwrap();
    let missing = Path::new("/nonexistent-wc-refresh-input");
    for views in [&[View::Raw][..], &View::ALL[..]] {
        let limit = if views.len() == 1 { 64 } else { 128 };
        let error = refresh_groups(&[data(missing, "new")], binding(), views, limit, Some(&old))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("profile mismatch"), "{error}");
    }
}

fn command(dir: &Path, reuse: Option<&Path>, files: &[DataFile]) -> std::process::Output {
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_wc-lake"));
    c.args([
        "refresh-groups",
        "--out-dir",
        dir.to_str().unwrap(),
        "--table-uuid",
        &binding().table_uuid,
        "--column",
        "message",
        "--field-id",
        "2",
        "--guard",
        "id",
        "--guard-field-id",
        "1",
        "--views",
        "raw,lower,decoded,revdomain,winpath",
        "--max-value-bytes",
        "64",
        "--snapshot-id",
        "23",
        "--sequence-number",
        "7",
    ]);
    if let Some(p) = reuse {
        c.args(["--reuse-groups", p.to_str().unwrap()]);
    }
    for f in files {
        c.arg(format!("{}={}", f.local, f.lake_path));
    }
    c.output().unwrap()
}

#[test]
fn cli_publishes_complete_pair_and_never_overwrites_existing_directory() {
    let t = Temp::new();
    let file = t.0.join("input.parquet");
    write(&file, vec![Some("powershell.exe")]);
    let out = t.0.join("first");
    let done = command(&out, None, &[data(&file, "a")]);
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["indexed_files"], 1);
    assert_eq!(manifest["reused_files"], 0);
    assert_eq!(manifest["snapshot_id"], 23);
    assert_eq!(
        manifest["paths"],
        serde_json::json!(["s3://warehouse/a.parquet"])
    );
    let group = GroupIndex::read(&fs::read(out.join("groups.puffin")).unwrap()).unwrap();
    let files =
        wildcard_puffin::read_indexes(&fs::read(out.join("files.puffin")).unwrap()).unwrap();
    assert_eq!(files[0].0.snapshot_id, 23);
    assert_eq!(files[0].1, group.file_index().unwrap());
    let before = fs::read(out.join("manifest.json")).unwrap();
    assert!(!command(&out, None, &[data(&file, "a")]).status.success());
    assert_eq!(fs::read(out.join("manifest.json")).unwrap(), before);
    fs::remove_file(&file).unwrap();
    let next = t.0.join("next");
    let done = command(&next, Some(&out.join("groups.puffin")), &[data(&file, "a")]);
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let next_manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(next.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(next_manifest["indexed_files"], 0);
    assert_eq!(next_manifest["reused_files"], 1);
    let corrupt = t.0.join("corrupt.puffin");
    fs::write(&corrupt, b"corrupt").unwrap();
    let bad = t.0.join("bad");
    assert!(!command(&bad, Some(&corrupt), &[]).status.success());
    assert!(!bad.join("manifest.json").exists());
}
