// SPDX-License-Identifier: Apache-2.0
//! Only inspects aggregate sidecar budgets; does not certify index contents.
use libwildcard::puffin::segments::{Budget, MAX_ARTIFACTS};
pub fn run(argv: &[String]) -> Result<(), String> {
    if argv.is_empty() || argv.len() > MAX_ARTIFACTS || argv.iter().any(|a| a.starts_with("--")) {
        return Err("segment-budget expects 1..192 Puffin paths and no options".into());
    }
    let mut budget = Budget::default();
    for path in argv {
        let bytes = crate::read_file_limited(path, budget.remaining_compressed_bytes())?;
        budget.charge(&bytes).map_err(|e| e.to_string())?;
    }
    println!(
        "{}",
        serde_json::json!({"version":1,"artifacts":budget.artifacts(),
        "compressed_bytes":budget.compressed_bytes(),"payload_bytes":budget.payload_bytes()})
    );
    Ok(())
}
