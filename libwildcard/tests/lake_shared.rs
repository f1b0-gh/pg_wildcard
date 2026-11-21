#![cfg(feature = "cold")]
// SPDX-License-Identifier: Apache-2.0
use arrow_array::{ArrayRef, BinaryArray, RecordBatch, StringArray, TimestampMicrosecondArray};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use libwildcard::core::View;
use libwildcard::lake::{
    DataFile, identity::index_identities, rowgroup::index_groups, shared::refresh_search,
};
use libwildcard::puffin::{identity, rowgroup};
use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
use std::{fs, path::PathBuf, sync::Arc};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("wc-shared-{name}-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, missing_id: bool, hostile: bool) -> DataFile {
        let mut raw: Vec<Option<Vec<u8>>> = (0..48)
            .map(|i| Some(format!("PowerShell.exe target-{i}").into_bytes()))
            .collect();
        let mut ids: Vec<Option<String>> =
            (0..48).map(|i| Some(format!("id-{}", i % 40))).collect();
        let mut ticks: Vec<Option<i64>> = (0..48).map(|i| Some(100 + i)).collect();
        if hostile {
            raw[0] = None;
            raw[1] = Some(vec![b'x'; 33]);
            raw[2] = Some(b"\xff\0%41\\Temp".to_vec());
            ids[3] = None;
            ids[4] = Some("z".repeat(257));
            ids[5] = Some("id'\\\0é".into());
            ticks[6] = None;
        }
        let mut fields = vec![
            Field::new(
                "ts",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                true,
            ),
            Field::new("message", DataType::Binary, true),
        ];
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(TimestampMicrosecondArray::from(ticks).with_timezone("UTC")),
            Arc::new(BinaryArray::from_iter(raw.iter().map(|v| v.as_deref()))),
        ];
        if !missing_id {
            fields.push(Field::new("event_id", DataType::Utf8, true));
            columns.push(Arc::new(StringArray::from(ids)));
        }
        let schema = Arc::new(Schema::new(fields));
        let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();
        let path = self.0.join(name);
        let options = WriterProperties::builder()
            .set_max_row_group_row_count(Some(11))
            .build();
        let mut writer =
            ArrowWriter::try_new(fs::File::create(&path).unwrap(), schema, Some(options)).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        DataFile {
            local: path.to_string_lossy().into(),
            lake_path: format!("s3://warehouse/{name}"),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn bindings() -> (rowgroup::Binding, identity::Binding) {
    let table = "00000000-0000-4000-8000-000000000001";
    (
        rowgroup::Binding {
            table_uuid: table.into(),
            column: "message".into(),
            guard: "ts".into(),
            column_field_id: 7,
            guard_field_id: 9,
            guard_type: rowgroup::GuardType::TimestampUsUtc,
        },
        identity::Binding {
            table_uuid: table.into(),
            column: "message".into(),
            guard: "event_id".into(),
            column_field_id: 7,
            guard_field_id: 3,
        },
    )
}
#[test]
fn shared_scan_matches_separate_builders_with_independent_fallbacks() {
    let fixture = Fixture::new("parity");
    let files = [
        fixture.write("ordinary.parquet", false, false),
        fixture.write("hostile.parquet", false, true),
        fixture.write("missing.parquet", true, false),
    ];
    let (g, i) = bindings();
    for views in [
        &View::ALL[..],
        &[View::Raw][..],
        &[View::WinPath, View::Decoded][..],
    ] {
        let groups = index_groups(&files, g.clone(), views, 32).unwrap();
        let ids = index_identities(&files, i.clone(), views, 32).unwrap();
        let joint = refresh_search(&files, g.clone(), i.clone(), views, 32, None).unwrap();
        assert_eq!(joint.indexed_files, 3);
        assert_eq!(joint.reused_files, 0);
        assert_eq!(
            joint.groups.write(42, 1).unwrap(),
            groups.write(42, 1).unwrap()
        );
        assert_eq!(
            joint.identities.write(42, 1).unwrap(),
            ids.write(42, 1).unwrap()
        );
        assert_eq!(
            joint.groups.file_index().unwrap(),
            groups.file_index().unwrap()
        );
    }
}
#[test]
fn paired_reuse_never_opens_old_inputs_and_rejects_mixed_caches() {
    let fixture = Fixture::new("reuse");
    let first = fixture.write("first.parquet", false, false);
    let removed = fixture.write("removed.parquet", false, true);
    let files = [first, removed];
    let (g, i) = bindings();
    let old = refresh_search(&files, g.clone(), i.clone(), &View::ALL, 32, None).unwrap();
    for f in &files {
        fs::remove_file(&f.local).unwrap();
    }
    let appended = fixture.write("appended.parquet", false, true);
    let current = [
        appended,
        DataFile {
            local: "/does-not-exist".into(),
            lake_path: files[0].lake_path.clone(),
        },
    ];
    let new = refresh_search(
        &current,
        g.clone(),
        i.clone(),
        &View::ALL,
        32,
        Some((&old.groups, &old.identities)),
    )
    .unwrap();
    assert_eq!(new.indexed_files, 1);
    assert_eq!(new.reused_files, 1);
    assert_eq!(new.groups.files()[0].path, current[0].lake_path);
    let mismatched = identity::Builder::new(i.clone(), &View::ALL, 32)
        .unwrap()
        .finish();
    assert!(
        refresh_search(
            &current,
            g.clone(),
            i.clone(),
            &View::ALL,
            32,
            Some((&old.groups, &mismatched))
        )
        .is_err()
    );
    let mut bad = i.clone();
    bad.table_uuid = "00000000-0000-4000-8000-000000000002".into();
    assert!(
        refresh_search(
            &current,
            g.clone(),
            bad,
            &View::ALL,
            32,
            Some((&old.groups, &old.identities))
        )
        .is_err()
    );
    assert!(
        refresh_search(
            &current,
            g.clone(),
            i.clone(),
            &[View::Raw],
            32,
            Some((&old.groups, &old.identities))
        )
        .is_err()
    );
    assert!(
        refresh_search(
            &current,
            g,
            i,
            &View::ALL,
            31,
            Some((&old.groups, &old.identities))
        )
        .is_err()
    );
}

#[test]
fn independent_missing_timestamp_and_wrong_column_types() {
    use arrow_array::Int64Array;
    let fixture = Fixture::new("types");
    let (g, i) = bindings();
    for (name, ts, id) in [
        ("missing-time", None, true),
        ("null-time", Some(true), true),
        ("bad-time", Some(false), true),
        ("bad-id", None, false),
    ] {
        let mut fields = vec![Field::new("message", DataType::Utf8, false)];
        let mut arrays: Vec<ArrayRef> =
            vec![Arc::new(StringArray::from(vec!["abc target", "ordinary"]))];
        if let Some(valid) = ts {
            if valid {
                fields.push(Field::new(
                    "ts",
                    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                    true,
                ));
                arrays.push(Arc::new(
                    TimestampMicrosecondArray::from(vec![None, Some(100)]).with_timezone("UTC"),
                ));
            } else {
                fields.push(Field::new("ts", DataType::Int64, false));
                arrays.push(Arc::new(Int64Array::from(vec![1, 2])));
            }
        }
        fields.push(Field::new(
            "event_id",
            if id { DataType::Utf8 } else { DataType::Int64 },
            false,
        ));
        arrays.push(if id {
            Arc::new(StringArray::from(vec!["first", "second"]))
        } else {
            Arc::new(Int64Array::from(vec![1, 2]))
        });
        let schema = Arc::new(Schema::new(fields));
        let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
        let local = fixture.0.join(name);
        let mut writer =
            ArrowWriter::try_new(fs::File::create(&local).unwrap(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let files = [DataFile {
            local: local.to_string_lossy().into(),
            lake_path: format!("s3://warehouse/{name}"),
        }];
        let joint = refresh_search(&files, g.clone(), i.clone(), &View::ALL, 32, None);
        if !id || ts == Some(false) {
            assert!(joint.is_err(), "{name}");
            continue;
        }
        let joint = joint.unwrap();
        assert_eq!(
            joint.groups.write(42, 1).unwrap(),
            index_groups(&files, g.clone(), &View::ALL, 32)
                .unwrap()
                .write(42, 1)
                .unwrap()
        );
        assert_eq!(
            joint.identities.write(42, 1).unwrap(),
            index_identities(&files, i.clone(), &View::ALL, 32)
                .unwrap()
                .write(42, 1)
                .unwrap()
        );
        let plan = joint
            .identities
            .plan(
                &i,
                libwildcard::core::verify::Kind::Wildcard,
                "*abc*",
                View::Raw,
            )
            .unwrap();
        assert!(
            !plan.full_scan,
            "{name}: sparse identity must survive absent timestamp guard"
        );
    }
}
