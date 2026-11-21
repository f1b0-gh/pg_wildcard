// SPDX-License-Identifier: Apache-2.0
//! Constant-only conversion for native Parquet range pushdown; never cast the
//! guarded column. Consumers must retain unknown files and exact predicates.
use crate::antalya::{ChError, ident, quote};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeType {
    Int64,
    TimestampUsUtc,
}
pub fn predicate(column: &str, ranges: &[(i64, i64)], kind: RangeType) -> Result<String, ChError> {
    if column.len() > 128 {
        return Err(ChError::Range);
    }
    let column = ident(column)?;
    if ranges.len() > 128 {
        return Err(ChError::Range);
    }
    let literal = |tick: i64| match kind {
        RangeType::Int64 => format!("toInt64({})", quote(&tick.to_string())),
        RangeType::TimestampUsUtc => format!(
            "fromUnixTimestamp64Micro(toInt64({}), 'UTC')",
            quote(&tick.to_string())
        ),
    };
    let mut parts = Vec::new();
    for &(lo, hi) in ranges {
        if lo > hi
            || (kind == RangeType::TimestampUsUtc
                && (lo < -2_208_988_800_000_000 || hi > 10_413_791_999_999_999))
        {
            return Err(ChError::Range);
        }
        parts.push(format!(
            "({column} BETWEEN {} AND {})",
            literal(lo),
            literal(hi)
        ));
    }
    Ok(if parts.is_empty() {
        "0".into()
    } else {
        parts.join(" OR ")
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamp_constants_keep_signed_ticks_and_column_untransformed() {
        let sql = predicate("ts", &[(-1_000_001, -1), (0, 1)], RangeType::TimestampUsUtc).unwrap();
        assert!(
            sql.contains("ts BETWEEN fromUnixTimestamp64Micro(toInt64('-1000001'), 'UTC')"),
            "{sql}"
        );
        assert!(sql.contains("fromUnixTimestamp64Micro(toInt64('-1'), 'UTC')"));
        assert!(predicate("ts); DROP TABLE logs", &[(0, 1)], RangeType::TimestampUsUtc).is_err());
        assert!(predicate("ts", &[(2, 1)], RangeType::TimestampUsUtc).is_err());
        assert!(predicate("ts", &[(i64::MIN, 1)], RangeType::TimestampUsUtc).is_err());
        assert!(predicate("ts", &vec![(0, 1); 129], RangeType::TimestampUsUtc).is_err());
        assert_eq!(
            predicate("ts", &[], RangeType::TimestampUsUtc).unwrap(),
            "0"
        );
        assert!(
            predicate("id", &[(i64::MIN, i64::MAX)], RangeType::Int64)
                .unwrap()
                .contains("-9223372036854775808")
        );
    }
}
