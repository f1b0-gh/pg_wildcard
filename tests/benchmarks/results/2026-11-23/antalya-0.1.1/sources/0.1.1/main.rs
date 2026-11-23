// SPDX-License-Identifier: Apache-2.0
//! Persistent bounded antalya-rs worker; exact pair hashing is done after timing.
use antalya_rs::{
    AntalyaClient, Config, Credentials, Endpoint, ExecutionLimits, Outcome, Query, TerminalPolicy,
    TerminalStrategy,
};
use arrow_array::{Array, BinaryArray, Int64Array, RecordBatch, StringArray, UInt64Array};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    time::{Duration, Instant},
};
type Pair = (i64, i64, String);
fn append_pairs(batch: &RecordBatch, out: &mut Vec<Pair>) -> Result<(), &'static str> {
    if batch.num_columns() != 3 || out.len().saturating_add(batch.num_rows()) > 100000 {
        return Err("pair shape/budget exceeded");
    }
    fn integer(array: &dyn Array, row: usize) -> Result<i64, &'static str> {
        if array.is_null(row) {
            return Err("null pair field");
        }
        if let Some(a) = array.as_any().downcast_ref::<Int64Array>() {
            return Ok(a.value(row));
        }
        if let Some(a) = array.as_any().downcast_ref::<UInt64Array>() {
            return a.value(row).try_into().map_err(|_| "pair integer overflow");
        }
        Err("pair integer type")
    }
    for row in 0..batch.num_rows() {
        let id = integer(batch.column(0).as_ref(), row)?;
        let indicator = integer(batch.column(1).as_ref(), row)?;
        let array = batch.column(2);
        if array.is_null(row) {
            return Err("null pair view");
        }
        let view = if let Some(a) = array.as_any().downcast_ref::<StringArray>() {
            a.value(row)
        } else if let Some(a) = array.as_any().downcast_ref::<BinaryArray>() {
            std::str::from_utf8(a.value(row)).map_err(|_| "pair view encoding")?
        } else {
            return Err("pair view type");
        };
        if !["raw", "lower", "decoded", "revdomain", "winpath"].contains(&view) {
            return Err("unknown pair view");
        }
        out.push((id, indicator, view.to_owned()));
    }
    Ok(())
}
async fn run(client: &AntalyaClient, command: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let queries = command["queries"].as_array().ok_or("queries")?;
    if queries.is_empty() || queries.len() > 256 {
        return Err("query count bound".into());
    }
    let mut pairs = Vec::new();
    let mut executions = Vec::new();
    let start = Instant::now();
    for sql in queries {
        let sql = sql.as_str().ok_or("SQL string")?;
        if sql.len() > 250000 {
            return Err("SQL byte bound".into());
        }
        let mut query = Query::new(sql, format!("comparison-direct-{}", uuid::Uuid::new_v4()))?;
        let database = command["database"].as_str().ok_or("database")?;
        query = query.database(database)?;
        for (k, v) in [
            ("max_threads", "4"),
            ("max_block_size", "8192"),
            ("max_result_rows", "100000"),
            ("max_result_bytes", "67108864"),
            ("result_overflow_mode", "throw"),
            ("max_memory_usage", "4000000000"),
            ("max_execution_time", "60"),
            ("use_query_cache", "0"),
            ("use_query_condition_cache", "0"),
            ("output_format_arrow_string_as_string", "0"),
            ("output_format_arrow_low_cardinality_as_dictionary", "0"),
            ("cancel_http_readonly_queries_on_client_close", "1"),
        ] {
            query = query.setting(k, v)?;
        }
        let mut execution = client.fetch_arrow(query).await?;
        let mut peak = 0;
        while let Some(batch) = execution.next_batch().await? {
            peak = peak.max(batch.charged_bytes());
            append_pairs(&batch, &mut pairs)?;
        }
        if execution.outcome() != Outcome::Succeeded {
            return Err("unconfirmed execution".into());
        }
        let m = execution.metrics();
        let ms = |d: Duration| d.as_secs_f64() * 1000.;
        executions.push(json!({"query_id":execution.query_id(),"wire_bytes":m.wire_bytes,"decode_ms":m.decode.map(ms),"body_wait_ms":m.transfer.map(ms),"terminal_ms":ms(m.terminal),"first_batch_ms":m.first_batch.map(ms),"peak_individual_batch_bytes":peak}));
    }
    Ok(
        json!({"request_ms":start.elapsed().as_secs_f64()*1000.,"pairs":pairs,"executions":executions}),
    )
}
#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let codec = std::env::args().nth(1).unwrap_or_else(|| "none".into());
    if !matches!(codec.as_str(), "none" | "zstd") { return Err("invalid codec".into()); }
    #[cfg(not(feature = "v011"))]
    if codec != "none" { return Err("0.1.0 only supports none".into()); }
    let config = Config::new(
            Endpoint::parse(&std::env::var("ANTALYA_TEST_URL")?)?,
            Credentials::bearer(&std::env::var("ANTALYA_TEST_TOKEN")?)?,
        )
        .timeout(Duration::from_secs(65))
        .terminal_policy(TerminalPolicy {
            strategy: TerminalStrategy::FlushFirst,
            ..Default::default()
        })
        .execution_limits(ExecutionLimits {
            max_batch_bytes: 16 << 20,
            ..Default::default()
        });
    #[cfg(feature = "v011")]
    let config = config.arrow_compression(if codec == "zstd" { antalya_rs::ArrowCompression::Zstd } else { antalya_rs::ArrowCompression::None });
    let client = AntalyaClient::new(config)?;
    println!(
        "{}",
        json!({"ready":true,"client_setup_ms":start.elapsed().as_secs_f64()*1000.,"client":env!("CARGO_PKG_VERSION"),"codec":codec})
    );
    io::stdout().flush()?;
    let mut input = io::stdin().lock();
    loop {
        let mut bytes = Vec::new();
        loop {
            let available = input.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let count = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |n| n + 1);
            if bytes.len() + count > 8 << 20 {
                return Err("command byte bound".into());
            }
            let done = available[count - 1] == b'\n';
            bytes.extend_from_slice(&available[..count]);
            input.consume(count);
            if done {
                break;
            }
        }
        if bytes.is_empty() {
            break;
        }
        let result = match serde_json::from_slice::<Value>(&bytes) {
            Ok(command) => {
                match tokio::time::timeout(Duration::from_secs(60), run(&client, &command)).await {
                    Ok(Ok(value)) => value,
                    _ => {
                        json!({"error":"direct execution failed (bounded or engine error); inspect correlated native log"})
                    }
                }
            }
            Err(_) => json!({"error":"invalid command"}),
        };
        println!("{result}");
        io::stdout().flush()?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Int64Array, StringArray};
    use std::sync::Arc;
    #[test]
    fn every_pair_value_is_consumed_and_nulls_rejected() {
        let batch = RecordBatch::try_from_iter(vec![
            ("id", Arc::new(Int64Array::from(vec![5, 9])) as _),
            (
                "indicator_id",
                Arc::new(Int64Array::from(vec![2, 999])) as _,
            ),
            (
                "view",
                Arc::new(StringArray::from(vec!["raw", "lower"])) as _,
            ),
        ])
        .unwrap();
        let mut out = Vec::new();
        append_pairs(&batch, &mut out).unwrap();
        assert_eq!(out, vec![(5, 2, "raw".into()), (9, 999, "lower".into())]);
        let bad = RecordBatch::try_from_iter(vec![
            ("id", Arc::new(Int64Array::from(vec![None])) as _),
            ("indicator_id", Arc::new(Int64Array::from(vec![2])) as _),
            ("view", Arc::new(StringArray::from(vec!["raw"])) as _),
        ])
        .unwrap();
        assert!(append_pairs(&bad, &mut out).is_err());
    }
    #[test]
    fn pair_limit_rejects_further_batches() {
        let batch = RecordBatch::try_from_iter(vec![
            ("id", Arc::new(Int64Array::from(vec![1])) as _),
            ("indicator_id", Arc::new(Int64Array::from(vec![2])) as _),
            ("view", Arc::new(StringArray::from(vec!["raw"])) as _),
        ])
        .unwrap();
        let mut out = vec![(0, 0, "raw".into()); 100000];
        assert!(append_pairs(&batch, &mut out).is_err());
    }
}
