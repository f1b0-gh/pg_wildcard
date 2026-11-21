// SPDX-License-Identifier: Apache-2.0
//! Projected Parquet adapter for sparse identities. Raw values stay unchanged.
use crate::core::View;
use crate::lake::{DataFile, LakeError};
use crate::puffin::identity::{Binding, Builder, IdentityIndex};
use arrow_array::{
    Array, BinaryArray, BinaryViewArray, LargeBinaryArray, LargeStringArray, StringArray,
    StringViewArray,
};
use arrow_schema::DataType;
use parquet::arrow::{ProjectionMask, arrow_reader::ParquetRecordBatchReaderBuilder};
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Lake(#[from] LakeError),
    #[error(transparent)]
    Index(#[from] crate::puffin::identity::Error),
    #[error("identity indexing: {0}")]
    Invalid(&'static str),
}
fn bytes(a: &dyn Array, row: usize) -> Result<Option<&[u8]>, Error> {
    if a.is_null(row) {
        return Ok(None);
    }
    macro_rules! value {
        ($ty:ty) => {
            Ok(Some(
                a.as_any()
                    .downcast_ref::<$ty>()
                    .ok_or(Error::Invalid("array type"))?
                    .value(row)
                    .as_ref(),
            ))
        };
    }
    match a.data_type() {
        DataType::Utf8 => value!(StringArray),
        DataType::LargeUtf8 => value!(LargeStringArray),
        DataType::Utf8View => value!(StringViewArray),
        DataType::Binary => value!(BinaryArray),
        DataType::LargeBinary => value!(LargeBinaryArray),
        DataType::BinaryView => value!(BinaryViewArray),
        _ => Err(Error::Invalid("unsupported array")),
    }
}
pub fn index_identities(
    files: &[DataFile],
    binding: Binding,
    views: &[View],
    max_value_bytes: usize,
) -> Result<IdentityIndex, Error> {
    let mut b = Builder::new(binding.clone(), views, max_value_bytes)?;
    // Validate complete operator configuration before opening any Parquet.
    for f in files {
        b.start_file(&f.lake_path)?;
    }
    let mut b = Builder::new(binding.clone(), views, max_value_bytes)?;
    for f in files {
        b.start_file(&f.lake_path)?;
        let reader = ParquetRecordBatchReaderBuilder::try_new(
            std::fs::File::open(&f.local).map_err(LakeError::from)?,
        )
        .map_err(LakeError::from)?;
        let schema = reader.schema();
        let message = schema
            .index_of(&binding.column)
            .map_err(|_| Error::Invalid("message column absent"))?;
        if !matches!(
            schema.field(message).data_type(),
            DataType::Utf8
                | DataType::LargeUtf8
                | DataType::Utf8View
                | DataType::Binary
                | DataType::LargeBinary
                | DataType::BinaryView
        ) {
            return Err(Error::Invalid("message type"));
        }
        let guard = schema.index_of(&binding.guard).ok();
        if guard.is_some_and(|i| {
            !matches!(
                schema.field(i).data_type(),
                DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
            )
        }) {
            return Err(Error::Invalid("guard must be UTF8 string"));
        }
        let mut columns = vec![message];
        if let Some(g) = guard {
            columns.push(g)
        }
        columns.sort_unstable();
        let mask = ProjectionMask::roots(reader.parquet_schema(), columns);
        let mut count = 0u64;
        let expected = u64::try_from(reader.metadata().file_metadata().num_rows())
            .map_err(|_| Error::Invalid("row count"))?;
        for batch in reader
            .with_projection(mask)
            .with_batch_size(8192)
            .build()
            .map_err(LakeError::from)?
        {
            let batch = batch.map_err(LakeError::from)?;
            let values = batch
                .column_by_name(&binding.column)
                .ok_or(Error::Invalid("projected message"))?;
            let guards = batch.column_by_name(&binding.guard);
            for i in 0..batch.num_rows() {
                let guard = guards
                    .map(|g| bytes(g.as_ref(), i))
                    .transpose()?
                    .flatten()
                    .map(std::str::from_utf8)
                    .transpose()
                    .map_err(|_| Error::Invalid("guard UTF8"))?;
                b.add_value(bytes(values.as_ref(), i)?, guard)?;
                count = count.checked_add(1).ok_or(Error::Invalid("row count"))?;
            }
        }
        if count != expected {
            return Err(Error::Invalid("incomplete Parquet rows"));
        }
    }
    Ok(b.finish())
}
/// Counters describe actual Parquet work; reused paths are never opened.
#[derive(Debug)]
pub struct Refresh {
    pub index: IdentityIndex,
    pub indexed_files: usize,
    pub reused_files: usize,
}

/// Caller must authenticate the cache, current inventory and immutable objects.
/// A changed object at the same path invalidates reuse, regardless of snapshot.
pub fn refresh_identities(
    files: &[DataFile],
    binding: Binding,
    views: &[View],
    max_value_bytes: usize,
    previous: Option<&IdentityIndex>,
) -> Result<Refresh, Error> {
    let mut config = Builder::new(binding.clone(), views, max_value_bytes)?;
    for file in files {
        config.start_file(&file.lake_path)?;
    }
    let config = config.finish();
    if let Some(old) = previous
        && (old.binding() != &binding
            || old.views() != config.views()
            || old.max_value_bytes() != max_value_bytes)
    {
        return Err(Error::Invalid("reuse profile mismatch"));
    }
    let known: std::collections::BTreeSet<_> = previous
        .into_iter()
        .flat_map(|old| old.files().iter().map(|f| f.path()))
        .collect();
    let new_files: Vec<_> = files
        .iter()
        .filter(|f| !known.contains(f.lake_path.as_str()))
        .map(|f| DataFile {
            local: f.local.clone(),
            lake_path: f.lake_path.clone(),
        })
        .collect();
    let fresh = index_identities(&new_files, binding, config.views(), max_value_bytes)?;
    let indexed_files = fresh.files().len();
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
        index,
        indexed_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Int64Array, RecordBatch};
    use arrow_schema::{Field, Schema};
    use parquet::arrow::ArrowWriter;
    use std::sync::Arc;
    fn binding() -> Binding {
        Binding {
            table_uuid: "00000000-0000-4000-8000-000000000001".into(),
            column: "message".into(),
            guard: "event_id".into(),
            column_field_id: 1,
            guard_field_id: 2,
        }
    }
    #[test]
    fn projected_adapter_preserves_strings_and_nullable_missing_guard_fallback() {
        let dir = std::env::temp_dir().join(format!("wc-identity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for case in 0..4 {
            let mut fields = vec![Field::new("message", DataType::Utf8, true)];
            let mut arrays: Vec<Arc<dyn Array>> = vec![Arc::new(StringArray::from(vec![
                Some("target"),
                Some("ordinary"),
            ]))];
            if case != 2 {
                let (ty, a): (DataType, Arc<dyn Array>) = if case == 3 {
                    (DataType::Int64, Arc::new(Int64Array::from(vec![1, 2])))
                } else {
                    (
                        DataType::Utf8,
                        Arc::new(StringArray::from(vec![
                            if case == 1 { None } else { Some("id'\\\0") },
                            Some("other"),
                        ])),
                    )
                };
                fields.push(Field::new("event_id", ty, true));
                arrays.push(a);
            }
            let schema = Arc::new(Schema::new(fields));
            let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
            let p = dir.join(format!("{case}.parquet"));
            let mut w =
                ArrowWriter::try_new(std::fs::File::create(&p).unwrap(), schema, None).unwrap();
            w.write(&batch).unwrap();
            w.close().unwrap();
            let result = index_identities(
                &[DataFile {
                    local: p.to_string_lossy().into(),
                    lake_path: "lake/file".into(),
                }],
                binding(),
                &[View::Raw],
                8192,
            );
            if case == 3 {
                assert!(result.is_err());
                continue;
            }
            let idx = result.unwrap();
            assert_eq!(idx.files()[0].rows(), 2);
            let plan = idx
                .plan(
                    &binding(),
                    crate::core::verify::Kind::Wildcard,
                    "*target*",
                    View::Raw,
                )
                .unwrap();
            if case == 0 {
                assert_eq!(plan.files[0].values, Some(vec!["id'\\\0".into()]))
            } else {
                assert!(plan.full_scan)
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
