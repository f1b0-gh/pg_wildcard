// SPDX-License-Identifier: Apache-2.0
use super::{Error, plan::MAX_PAIRS};
use arrow_array::{Array, BinaryArray, Int64Array, RecordBatch, StringArray, UInt64Array};
use std::sync::atomic::{AtomicUsize, Ordering};
pub type Pair = (i64, i64, String);
fn integer(a: &dyn Array, row: usize) -> Result<i64, Error> {
    if a.is_null(row) {
        return Err(Error::Invalid("null pair integer"));
    }
    if let Some(a) = a.as_any().downcast_ref::<Int64Array>() {
        return Ok(a.value(row));
    }
    if let Some(a) = a.as_any().downcast_ref::<UInt64Array>() {
        return a
            .value(row)
            .try_into()
            .map_err(|_| Error::Invalid("integer overflow"));
    }
    Err(Error::Invalid("pair integer type"))
}
pub fn append(batch: &RecordBatch, out: &mut Vec<Pair>, total: &AtomicUsize) -> Result<(), Error> {
    if batch.num_columns() != 3 {
        return Err(Error::Invalid("pair columns"));
    }
    total
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            n.checked_add(batch.num_rows()).filter(|n| *n <= MAX_PAIRS)
        })
        .map_err(|_| Error::Invalid("aggregate pair limit"))?;
    out.try_reserve_exact(batch.num_rows())
        .map_err(|_| Error::Invalid("pair allocation"))?;
    for row in 0..batch.num_rows() {
        let a = batch.column(2);
        if a.is_null(row) {
            return Err(Error::Invalid("null view"));
        }
        let view = if let Some(a) = a.as_any().downcast_ref::<StringArray>() {
            a.value(row)
        } else if let Some(a) = a.as_any().downcast_ref::<BinaryArray>() {
            std::str::from_utf8(a.value(row)).map_err(|_| Error::Invalid("view encoding"))?
        } else {
            return Err(Error::Invalid("view type"));
        };
        if !["raw", "lower", "decoded", "revdomain", "winpath"].contains(&view) {
            return Err(Error::Invalid("unknown view"));
        }
        out.push((
            integer(batch.column(0).as_ref(), row)?,
            integer(batch.column(1).as_ref(), row)?,
            view.into(),
        ));
    }
    Ok(())
}
pub fn counts(batch: &RecordBatch) -> Result<(u64, u64), Error> {
    if batch.num_columns() != 2 || batch.num_rows() != 1 {
        return Err(Error::Invalid("candidate count shape"));
    }
    let value = |col| -> Result<u64, Error> {
        let a = batch
            .column(col)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or(Error::Invalid("candidate count type"))?;
        if a.is_null(0) {
            return Err(Error::Invalid("null candidate count"));
        }
        Ok(a.value(0))
    };
    Ok((value(0)?, value(1)?))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn batch() -> RecordBatch {
        RecordBatch::try_from_iter(vec![
            ("id", Arc::new(Int64Array::from(vec![7, 7])) as _),
            ("indicator", Arc::new(Int64Array::from(vec![9, 9])) as _),
            ("view", Arc::new(StringArray::from(vec!["raw", "raw"])) as _),
        ])
        .unwrap()
    }
    #[test]
    fn aggregate_budget_is_shared_and_duplicates_survive() {
        let total = AtomicUsize::new(MAX_PAIRS - 2);
        let mut a = Vec::new();
        append(&batch(), &mut a, &total).unwrap();
        assert_eq!(a, vec![(7, 9, "raw".into()); 2]);
        assert!(append(&batch(), &mut Vec::new(), &total).is_err());
    }
    #[test]
    fn refuses_wrong_count_shape() {
        assert!(counts(&batch()).is_err());
    }
}
