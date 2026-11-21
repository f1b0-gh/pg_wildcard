// SPDX-License-Identifier: Apache-2.0
//! Shared end boundary: successful result verification precedes clock sampling.
use super::{Error, Result, uint};
use serde_json::Value;
use std::time::Duration;

pub(super) const CLOCK_BOUNDARY: &str = "request-construction-through-verified-result-v2";

pub(super) fn finish_verified(
    command: &Value,
    rows: usize,
    digest: &str,
    elapsed: impl FnOnce() -> Duration,
) -> Result<Duration> {
    if command["expected_digest"] != digest || uint(&command["expected_rows"])? != rows as u64 {
        return Err(Error::Values);
    }
    Ok(elapsed())
}
