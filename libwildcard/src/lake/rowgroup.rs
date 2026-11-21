// SPDX-License-Identifier: Apache-2.0
//! Experimental complete row-group indexing for top-level byte strings and an
//! existing signed Int64 guard. Missing/nullable guard data never excludes rows.
mod postings;

use crate::core::View;
use crate::lake::{DataFile, LakeError};
use crate::puffin::rowgroup::{Binding, GroupIndex};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Lake(#[from] LakeError),
    #[error(transparent)]
    Index(#[from] crate::puffin::rowgroup::Error),
    #[error(transparent)]
    Identity(#[from] crate::puffin::identity::Error),
    #[error("row-group indexing: {0}")]
    Invalid(&'static str),
}
pub fn index_groups(
    files: &[DataFile],
    binding: Binding,
    views: &[View],
    max_value_bytes: usize,
) -> Result<GroupIndex, Error> {
    index_groups_shared(files, binding, views, max_value_bytes, None)
}

pub(crate) fn index_groups_shared(
    files: &[DataFile],
    binding: Binding,
    views: &[View],
    max_value_bytes: usize,
    mut identities: Option<(
        &crate::puffin::identity::Binding,
        &mut crate::puffin::identity::Builder,
    )>,
) -> Result<GroupIndex, Error> {
    use crate::core::grams::{GramKey, value_gram_iter};
    use crate::puffin::{
        ngram::{FileEntry, NgramIndex},
        rowgroup::{
            DataFile as IndexedFile, Group, GuardType, MAX_FILES, MAX_GROUPS, MAX_PATH_BYTES,
            MAX_POSTINGS,
        },
    };
    use arrow_array::{
        Array, BinaryArray, BinaryViewArray, Int64Array, LargeBinaryArray, LargeStringArray,
        StringArray, StringViewArray, TimestampMicrosecondArray,
    };
    use parquet::arrow::{ProjectionMask, arrow_reader::ParquetRecordBatchReaderBuilder};
    use std::{collections::BTreeMap, fs::File};
    if files.len() > MAX_FILES
        || files.iter().map(|f| f.lake_path.len()).sum::<usize>() > MAX_PATH_BYTES
    {
        return Err(Error::Invalid("file/path budget"));
    }
    // Validate operator configuration before opening potentially large inputs.
    let config = GroupIndex::new(
        binding.clone(),
        files
            .iter()
            .map(|f| IndexedFile {
                path: f.lake_path.clone(),
                groups: 0,
            })
            .collect(),
        vec![],
        NgramIndex {
            views: views.to_vec(),
            files: vec![],
            grams: vec![],
        },
        max_value_bytes,
    )?;
    let views = &config.grams().views;
    let mut indexed = Vec::new();
    let mut groups = Vec::new();
    let mut entries = Vec::new();
    let mut grams: BTreeMap<GramKey, Vec<u32>> = BTreeMap::new();
    let mut postings = 0usize;
    let identity_guard = identities
        .as_ref()
        .map(|(binding, _)| binding.guard.clone());
    for (file_no, file) in files.iter().enumerate() {
        if let Some((_, builder)) = identities.as_mut() {
            builder.start_file(&file.lake_path)?;
        }
        let mut file_rows = 0u64;
        let reader = ParquetRecordBatchReaderBuilder::try_new(
            File::open(&file.local).map_err(LakeError::from)?,
        )
        .map_err(LakeError::from)?;
        let count = reader.metadata().num_row_groups();
        if count > MAX_GROUPS - groups.len() {
            return Err(Error::Invalid("group budget"));
        }
        let schema = reader.schema();
        let message = schema
            .index_of(&binding.column)
            .map_err(|_| Error::Invalid("message column absent"))?;
        if !matches!(
            schema.field(message).data_type(),
            arrow_schema::DataType::Utf8
                | arrow_schema::DataType::LargeUtf8
                | arrow_schema::DataType::Utf8View
                | arrow_schema::DataType::Binary
                | arrow_schema::DataType::LargeBinary
                | arrow_schema::DataType::BinaryView
        ) {
            return Err(Error::Invalid("message must be string or binary"));
        }
        let guard = schema.index_of(&binding.guard).ok();
        if let Some(i) = guard {
            let ty = schema.field(i).data_type();
            let valid = match binding.guard_type {
                GuardType::Int64 => ty == &arrow_schema::DataType::Int64,
                GuardType::TimestampUsUtc => {
                    matches!(ty,arrow_schema::DataType::Timestamp(arrow_schema::TimeUnit::Microsecond,Some(zone)) if zone.as_ref()=="UTC")
                }
            };
            if !valid {
                return Err(Error::Invalid("guard type/unit/timezone mismatch"));
            }
        }
        let mut columns = vec![message];
        if let Some(i) = guard {
            columns.push(i);
        }
        if let Some(name) = identity_guard.as_deref()
            && let Ok(i) = schema.index_of(name)
        {
            if !matches!(
                schema.field(i).data_type(),
                arrow_schema::DataType::Utf8
                    | arrow_schema::DataType::LargeUtf8
                    | arrow_schema::DataType::Utf8View
            ) {
                return Err(Error::Invalid("identity guard must be UTF8 string"));
            }
            columns.push(i);
        }
        columns.sort_unstable();
        columns.dedup();
        let mask = ProjectionMask::roots(reader.parquet_schema(), columns);
        indexed.push(IndexedFile {
            path: file.lake_path.clone(),
            groups: count as u32,
        });
        for ordinal in 0..count {
            let expected = reader.metadata().row_group(ordinal).num_rows();
            if expected < 0 {
                return Err(Error::Invalid("negative row count"));
            }
            let group = groups.len() as u32;
            let mut pending = postings::PendingGrams::new(MAX_POSTINGS - postings);
            let mut entry = FileEntry {
                path: group.to_string(),
                record_count: 0,
                has_unindexed: false,
            };
            let mut bounds: Option<(i64, i64)> = None;
            let mut unknown_guard = guard.is_none();
            let group_reader = ParquetRecordBatchReaderBuilder::try_new(
                File::open(&file.local).map_err(LakeError::from)?,
            )
            .map_err(LakeError::from)?
            .with_row_groups(vec![ordinal])
            .with_projection(mask.clone())
            .with_batch_size(8192)
            .build()
            .map_err(LakeError::from)?;
            for batch in group_reader {
                let batch = batch.map_err(LakeError::from)?;
                let col = batch
                    .column_by_name(&binding.column)
                    .ok_or(Error::Invalid("projected message missing"))?;
                let guards = batch.column_by_name(&binding.guard);
                for row in 0..batch.num_rows() {
                    entry.record_count += 1;
                    let tick = guards
                        .filter(|col| !col.is_null(row))
                        .and_then(|col| match binding.guard_type {
                            GuardType::Int64 => col
                                .as_any()
                                .downcast_ref::<Int64Array>()
                                .map(|a| a.value(row)),
                            GuardType::TimestampUsUtc => col
                                .as_any()
                                .downcast_ref::<TimestampMicrosecondArray>()
                                .map(|a| a.value(row)),
                        });
                    if let Some(tick) = tick.filter(|t| binding.guard_type.permits(*t)) {
                        bounds = Some(
                            bounds.map_or((tick, tick), |(lo, hi)| (lo.min(tick), hi.max(tick))),
                        );
                    } else {
                        unknown_guard = true;
                    }
                    if col.is_null(row) {
                        if let Some((_, builder)) = identities.as_mut() {
                            builder.add_value(None, None)?;
                        }
                        continue;
                    }
                    macro_rules! bytes {
                        ($ty:ty) => {
                            col.as_any()
                                .downcast_ref::<$ty>()
                                .ok_or(Error::Invalid("array type"))?
                                .value(row)
                                .as_ref()
                        };
                    }
                    let value: &[u8] = match col.data_type() {
                        arrow_schema::DataType::Utf8 => bytes!(StringArray),
                        arrow_schema::DataType::LargeUtf8 => bytes!(LargeStringArray),
                        arrow_schema::DataType::Utf8View => bytes!(StringViewArray),
                        arrow_schema::DataType::Binary => bytes!(BinaryArray),
                        arrow_schema::DataType::LargeBinary => bytes!(LargeBinaryArray),
                        arrow_schema::DataType::BinaryView => bytes!(BinaryViewArray),
                        _ => return Err(Error::Invalid("message must be string or binary")),
                    };
                    if value.len() > max_value_bytes {
                        entry.has_unindexed = true;
                    }
                    if let Some((_, builder)) = identities.as_mut() {
                        let guards = identity_guard
                            .as_deref()
                            .and_then(|name| batch.column_by_name(name));
                        let id = match guards.filter(|g| !g.is_null(row)) {
                            None => None,
                            Some(g) => {
                                macro_rules! id {
                                    ($ty:ty) => {
                                        Some(
                                            g.as_any()
                                                .downcast_ref::<$ty>()
                                                .ok_or(Error::Invalid("identity array"))?
                                                .value(row),
                                        )
                                    };
                                }
                                match g.data_type() {
                                    arrow_schema::DataType::Utf8 => id!(StringArray),
                                    arrow_schema::DataType::LargeUtf8 => id!(LargeStringArray),
                                    arrow_schema::DataType::Utf8View => id!(StringViewArray),
                                    _ => {
                                        return Err(Error::Invalid(
                                            "identity guard must be UTF8 string",
                                        ));
                                    }
                                }
                            }
                        };
                        let mut error = None;
                        builder.add_value_visiting_grams(Some(value), id, |gram| {
                            if error.is_none() {
                                error = pending.insert(gram).err();
                            }
                        })?;
                        if let Some(error) = error {
                            return Err(error);
                        }
                    } else if value.len() <= max_value_bytes {
                        for &view in views {
                            for gram in value_gram_iter(view, &view.apply(value)) {
                                pending.insert(gram)?;
                            }
                        }
                    }
                }
            }
            if entry.record_count != expected as u64 {
                return Err(Error::Invalid("incomplete row group"));
            }
            // Each gram occurs once in this complete group. The BTreeMap
            // restores canonical gram ordering regardless of HashSet order.
            for gram in pending.into_iter() {
                grams.entry(gram).or_default().push(group);
                postings += 1;
            }
            file_rows = file_rows
                .checked_add(entry.record_count)
                .ok_or(Error::Invalid("row count overflow"))?;
            entries.push(entry);
            groups.push(Group {
                file: file_no as u32,
                ordinal: ordinal as u32,
                bounds: if unknown_guard { None } else { bounds },
            });
        }
        if identities.is_some()
            && u64::try_from(reader.metadata().file_metadata().num_rows()).ok() != Some(file_rows)
        {
            return Err(Error::Invalid("incomplete Parquet rows"));
        }
    }
    Ok(GroupIndex::new(
        binding,
        indexed,
        groups,
        NgramIndex {
            views: views.to_vec(),
            files: entries,
            grams: grams.into_iter().collect(),
        },
        max_value_bytes,
    )?)
}

/// A complete refreshed index and explicit work counters. Reuse assumes trusted
/// provenance and immutable lake paths within an identical table/schema profile.
pub struct Refresh {
    pub index: GroupIndex,
    pub indexed_files: usize,
    pub reused_files: usize,
    pub indexed_groups: usize,
    pub reused_groups: usize,
}

pub fn refresh_groups(
    files: &[DataFile],
    binding: Binding,
    views: &[View],
    max_value_bytes: usize,
    previous: Option<&GroupIndex>,
) -> Result<Refresh, Error> {
    use crate::puffin::{ngram::NgramIndex, rowgroup::DataFile as IndexedFile};
    use std::collections::BTreeSet;
    // Validate the complete requested inventory/profile before opening inputs.
    let config = GroupIndex::new(
        binding.clone(),
        files
            .iter()
            .map(|f| IndexedFile {
                path: f.lake_path.clone(),
                groups: 0,
            })
            .collect(),
        vec![],
        NgramIndex {
            views: views.to_vec(),
            files: vec![],
            grams: vec![],
        },
        max_value_bytes,
    )?;
    if let Some(old) = previous
        && (old.binding() != &binding
            || old.grams().views != config.grams().views
            || old.max_value_bytes() != max_value_bytes)
    {
        return Err(Error::Invalid("reuse profile mismatch"));
    }
    let known: BTreeSet<_> = previous
        .into_iter()
        .flat_map(|old| old.files().iter().map(|f| f.path.as_str()))
        .collect();
    let new_files: Vec<_> = files
        .iter()
        .filter(|f| !known.contains(f.lake_path.as_str()))
        .map(|f| DataFile {
            local: f.local.clone(),
            lake_path: f.lake_path.clone(),
        })
        .collect();
    let fresh = index_groups(&new_files, binding, &config.grams().views, max_value_bytes)?;
    let indexed_files = fresh.files().len();
    let indexed_groups = fresh.groups().len();
    let index = if let Some(old) = previous {
        old.assemble(
            &fresh,
            &files
                .iter()
                .map(|f| f.lake_path.clone())
                .collect::<Vec<_>>(),
        )?
    } else {
        fresh
    };
    Ok(Refresh {
        reused_files: index.files().len() - indexed_files,
        reused_groups: index.groups().len() - indexed_groups,
        indexed_files,
        indexed_groups,
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::verify::Kind;
    use crate::puffin::rowgroup::PlanBudget;
    use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
    use std::{fs::File, sync::Arc};
    fn binding() -> Binding {
        Binding {
            table_uuid: "00000000-0000-0000-0000-000000000001".into(),
            column: "message".into(),
            guard: "id".into(),
            column_field_id: 2,
            guard_field_id: 1,
            guard_type: crate::puffin::rowgroup::GuardType::Int64,
        }
    }
    fn write_fixture(
        ids: Option<Vec<Option<i64>>>,
        values: Vec<Option<&str>>,
        suffix: &str,
    ) -> (std::path::PathBuf, DataFile) {
        let path = std::env::temp_dir().join(format!(
            "wc_groups_{}_{}.parquet",
            std::process::id(),
            suffix
        ));
        let mut fields = vec![Field::new("message", DataType::Utf8, true)];
        let mut cols: Vec<ArrayRef> = vec![Arc::new(StringArray::from(values))];
        if let Some(ids) = ids {
            fields.push(Field::new("id", DataType::Int64, true));
            cols.push(Arc::new(Int64Array::from(ids)));
        }
        let schema = Arc::new(Schema::new(fields));
        let batch = RecordBatch::try_new(schema.clone(), cols).unwrap();
        let props = WriterProperties::builder()
            .set_max_row_group_row_count(Some(2))
            .build();
        let mut writer =
            ArrowWriter::try_new(File::create(&path).unwrap(), schema, Some(props)).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let data = DataFile {
            local: path.to_string_lossy().into(),
            lake_path: "bucket/file.parquet".into(),
        };
        (path, data)
    }
    #[test]
    fn computes_ranges_from_parquet_and_roundtrips_without_oracle() {
        let (path, file) = write_fixture(
            Some(vec![
                Some(-10),
                Some(0),
                Some(100),
                Some(101),
                Some(i64::MAX),
            ]),
            vec![
                Some("benign"),
                None,
                Some("powershell.exe"),
                Some("other"),
                Some("benign"),
            ],
            "ranges",
        );
        let b = binding();
        let idx = index_groups(&[file], b.clone(), &[View::Raw], 8192).unwrap();
        assert_eq!(idx.groups().len(), 3);
        assert_eq!(idx.groups()[1].bounds, Some((100, 101)));
        let bytes = idx.write(1, 1).unwrap();
        let got = GroupIndex::read(&bytes).unwrap();
        assert_eq!(got, idx);
        let plan = got
            .plan(
                &b,
                Kind::Wildcard,
                "*powershell*",
                View::Raw,
                PlanBudget::default(),
            )
            .unwrap();
        assert_eq!(plan.files[0].ranges, Some(vec![(100, 101)]));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn null_missing_guard_and_long_values_are_not_lost() {
        let long = "x".repeat(100000);
        for (suffix, ids) in [("null", Some(vec![None, Some(10)])), ("missing", None)] {
            let (path, file) = write_fixture(ids, vec![Some(&long), Some("benign")], suffix);
            let b = binding();
            let idx = index_groups(&[file], b.clone(), &[View::Raw], 32).unwrap();
            let plan = idx
                .plan(
                    &b,
                    Kind::Wildcard,
                    "*needle*",
                    View::Raw,
                    PlanBudget::default(),
                )
                .unwrap();
            assert_eq!(plan.files[0].ranges, None);
            std::fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn timestamp_ticks_preserve_negative_subseconds_and_unknown_ranges() {
        use crate::puffin::rowgroup::{GuardType, MIN_TIMESTAMP_US};
        use arrow_array::TimestampMicrosecondArray;
        use arrow_schema::TimeUnit;
        let path =
            std::env::temp_dir().join(format!("wc_timestamps_{}.parquet", std::process::id()));
        let schema = Arc::new(Schema::new(vec![
            Field::new("message", DataType::Utf8, true),
            Field::new(
                "ts",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                true,
            ),
        ]));
        let messages = StringArray::from(vec![
            "powershell.exe",
            "benign",
            "other",
            "powershell.exe",
            "powershell.exe",
            "benign",
        ]);
        let timestamps = TimestampMicrosecondArray::from(vec![
            Some(-1_000_001),
            Some(-1),
            Some(0),
            Some(1),
            Some(MIN_TIMESTAMP_US - 1),
            Some(7),
        ])
        .with_timezone("UTC");
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(messages), Arc::new(timestamps)],
        )
        .unwrap();
        let props = WriterProperties::builder()
            .set_max_row_group_row_count(Some(2))
            .build();
        let mut writer =
            ArrowWriter::try_new(File::create(&path).unwrap(), schema, Some(props)).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let file = DataFile {
            local: path.to_string_lossy().into(),
            lake_path: "bucket/timestamp.parquet".into(),
        };
        let mut b = binding();
        b.guard = "ts".into();
        b.guard_type = GuardType::TimestampUsUtc;
        let idx = index_groups(&[file], b.clone(), &[View::Raw], 8192).unwrap();
        assert_eq!(idx.groups()[0].bounds, Some((-1_000_001, -1)));
        assert_eq!(idx.groups()[1].bounds, Some((0, 1)));
        assert_eq!(idx.groups()[2].bounds, None);
        assert_eq!(
            idx.plan(
                &b,
                Kind::Wildcard,
                "*powershell*",
                View::Raw,
                PlanBudget::default()
            )
            .unwrap()
            .files[0]
                .ranges,
            None
        );
        assert_eq!(GroupIndex::read(&idx.write(1, 1).unwrap()).unwrap(), idx);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn timestamp_guard_rejects_wrong_unit_timezone_and_int64_reinterpretation() {
        use crate::puffin::rowgroup::GuardType;
        use arrow_array::{TimestampMicrosecondArray, TimestampMillisecondArray};
        use arrow_schema::TimeUnit;
        let cases: Vec<(DataType, ArrayRef)> = vec![
            (
                DataType::Timestamp(TimeUnit::Microsecond, Some("Europe/Paris".into())),
                Arc::new(TimestampMicrosecondArray::from(vec![1]).with_timezone("Europe/Paris")),
            ),
            (
                DataType::Timestamp(TimeUnit::Microsecond, None),
                Arc::new(TimestampMicrosecondArray::from(vec![1])),
            ),
            (
                DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
                Arc::new(TimestampMillisecondArray::from(vec![1]).with_timezone("UTC")),
            ),
            (DataType::Int64, Arc::new(Int64Array::from(vec![1]))),
        ];
        for (i, (ty, values)) in cases.into_iter().enumerate() {
            let path = std::env::temp_dir().join(format!(
                "wc_bad_timestamp_{}_{}.parquet",
                std::process::id(),
                i
            ));
            let schema = Arc::new(Schema::new(vec![
                Field::new("message", DataType::Utf8, true),
                Field::new("ts", ty, true),
            ]));
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![Arc::new(StringArray::from(vec!["powershell.exe"])), values],
            )
            .unwrap();
            let mut writer =
                ArrowWriter::try_new(File::create(&path).unwrap(), schema, None).unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
            let file = DataFile {
                local: path.to_string_lossy().into(),
                lake_path: "bucket/invalid.parquet".into(),
            };
            let mut b = binding();
            b.guard = "ts".into();
            b.guard_type = GuardType::TimestampUsUtc;
            assert!(index_groups(&[file], b, &[View::Raw], 8192).is_err());
            std::fs::remove_file(path).unwrap();
        }
    }
}
