// SPDX-License-Identifier: Apache-2.0
//! Candidate guard only: callers must preserve exact predicates and unknown files.
use crate::antalya::{ChError, ident, quote};
pub fn predicate(column: &str, values: Option<&[String]>) -> Result<String, ChError> {
    if column.len() > 128 {
        return Err(ChError::Range);
    }
    let column = ident(column)?;
    let Some(values) = values else {
        return Ok("1".into());
    };
    if values.len() > 128 || values.iter().any(|v| v.len() > 256) {
        return Err(ChError::Range);
    }
    Ok(if values.is_empty() {
        "0".into()
    } else {
        format!(
            "{column} IN ({})",
            values
                .iter()
                .map(|v| quote(v))
                .collect::<Vec<_>>()
                .join(",")
        )
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_bounded_quoted_string_guards() {
        assert_eq!(predicate("event_id", None).unwrap(), "1");
        assert_eq!(predicate("event_id", Some(&[])).unwrap(), "0");
        assert_eq!(
            predicate("event_id", Some(&["x'\\\0".into()])).unwrap(),
            "event_id IN ('x\\'\\\\\\x00')"
        );
        assert!(predicate("x);drop", None).is_err());
        assert!(predicate("id", Some(&vec!["x".into(); 129])).is_err());
        assert!(predicate("id", Some(&["x".repeat(257)])).is_err());
    }
}
