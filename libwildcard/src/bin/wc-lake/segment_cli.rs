// SPDX-License-Identifier: Apache-2.0
//! Only inspects aggregate sidecar budgets; does not certify index contents.
use libwildcard::puffin::segments::{Budget, MAX_ARTIFACTS};
use std::io::Read;
pub fn run(argv: &[String]) -> Result<(), String> {
    if argv.is_empty() || argv.len() > MAX_ARTIFACTS || argv.iter().any(|a| a.starts_with("--")) {
        return Err("segment-budget expects 1..192 Puffin paths and no options".into());
    }
    let mut budget = Budget::default();
    for path in argv {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| "cannot open segmented artifact")?
            .take(budget.remaining_compressed_bytes() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read segmented artifact")?;
        budget.charge(&bytes).map_err(|e| e.to_string())?;
    }
    println!(
        "{}",
        serde_json::json!({"version":1,"artifacts":budget.artifacts(),
        "compressed_bytes":budget.compressed_bytes(),"payload_bytes":budget.payload_bytes()})
    );
    Ok(())
}
