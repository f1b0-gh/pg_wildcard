// SPDX-License-Identifier: Apache-2.0
//! Fixed typed time parameters for the owned historical comparison.
use super::{Error, Result};
use serde_json::Value;

pub(super) fn scope_parameters(command: &Value) -> Result<Vec<(String, String)>> {
    if command
        .get("settings")
        .and_then(Value::as_object)
        .is_some_and(|settings| settings.keys().any(|key| key.starts_with("param_")))
    {
        return Err(Error::Input);
    }
    let Some(parameters) = command.get("parameters") else {
        return Ok(Vec::new());
    };
    let parameters = parameters.as_object().ok_or(Error::Input)?;
    if parameters.len() != 2 {
        return Err(Error::Input);
    }
    ["scope_from", "scope_to"]
        .into_iter()
        .map(|key| {
            let value = parameters
                .get(key)
                .and_then(Value::as_str)
                .ok_or(Error::Input)?;
            if value.len() != 26
                || !value.bytes().enumerate().all(|(index, byte)| match index {
                    4 | 7 => byte == b'-',
                    10 => byte == b' ',
                    13 | 16 => byte == b':',
                    19 => byte == b'.',
                    _ => byte.is_ascii_digit(),
                })
            {
                return Err(Error::Input);
            }
            // The server's DateTime64 placeholder validates calendar semantics.
            // Restricted digits/punctuation need no ClickHouse string escaping.
            Ok((key.to_owned(), value.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn optional_or_complete_scope_parameters() {
        assert!(scope_parameters(&json!({})).unwrap().is_empty());
        let values = json!({"parameters":{"scope_from":"2026-11-01 00:00:00.000000","scope_to":"2026-11-11 00:00:00.000000"}});
        let parsed = scope_parameters(&values).unwrap();
        assert_eq!(
            parsed,
            vec![
                ("scope_from".into(), "2026-11-01 00:00:00.000000".into()),
                ("scope_to".into(), "2026-11-11 00:00:00.000000".into())
            ]
        );
    }
    #[test]
    fn scope_parameters_cannot_be_smuggled_as_settings() {
        let command = json!({"settings":{"param_scope_from":"injected"}});
        assert!(scope_parameters(&command).is_err());
    }
    #[test]
    fn malformed_names_shapes_and_timestamp_payloads_refuse() {
        for parameters in [
            json!(null),
            json!([]),
            json!({}),
            json!({"scope_from":"2026-11-01 00:00:00.000000"}),
            json!({"scope_from":1,"scope_to":"2026-11-11 00:00:00.000000"}),
            json!({"query_id":"x","scope_to":"2026-11-11 00:00:00.000000"}),
        ] {
            assert!(scope_parameters(&json!({"parameters":parameters})).is_err());
        }
        for value in [
            "",
            "2026-11-01T00:00:00.000000",
            "2026-11-01 00:00:00",
            "2026-11-01 00:00:00.00000é",
            "2026-11-01 00:00:00.000000'",
            "2026-11-01 00:00:00.00000\0",
            "2026-11-01 00:00:00.0000\\N",
        ] {
            assert!(scope_parameters(&json!({"parameters":{"scope_from":value,"scope_to":"2026-11-11 00:00:00.000000"}})).is_err());
        }
    }
}
