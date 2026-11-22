// SPDX-License-Identifier: Apache-2.0
//! Persistent direct-client FDW profiling worker; exact controls are untimed.
use antalya_rs::{
    AntalyaClient, Config, Credentials, Endpoint, ExecutionLimits, Outcome, Query, TerminalPolicy,
    TerminalStrategy,
};
use arrow_array::{Array, BinaryArray, Int64Array, RecordBatch, StringArray};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    time::{Duration, Instant},
};

struct ValueOracle {
    workload: String,
    seen: Vec<bool>,
    count: usize,
}
impl ValueOracle {
    fn new(workload: &str, rows: usize) -> Result<Self, &'static str> {
        if ![
            "narrow",
            "smalltext",
            "wide1k",
            "broad_recheck",
            "selective",
        ]
        .contains(&workload)
            || rows == 0
            || rows > 1_000_000
            || (workload == "selective" && rows > 1000)
        {
            return Err("invalid fixture bounds");
        }
        Ok(Self {
            workload: workload.into(),
            seen: vec![false; rows],
            count: 0,
        })
    }
    fn check(&mut self, batch: &RecordBatch) -> Result<(), &'static str> {
        let columns = if self.workload == "narrow" { 1 } else { 3 };
        if batch.num_columns() != columns {
            return Err("column count differs");
        }
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or("ID type differs")?;
        fn bytes(a: &dyn Array, row: usize) -> Result<&[u8], &'static str> {
            if a.is_null(row) {
                return Err("null fixture field");
            }
            if let Some(a) = a.as_any().downcast_ref::<BinaryArray>() {
                return Ok(a.value(row));
            }
            if let Some(a) = a.as_any().downcast_ref::<StringArray>() {
                return Ok(a.value(row).as_bytes());
            }
            Err("text type differs")
        }
        for row in 0..batch.num_rows() {
            if ids.is_null(row) {
                return Err("null fixture ID");
            }
            let id = usize::try_from(ids.value(row)).map_err(|_| "negative fixture ID")?;
            let stride = if self.workload == "selective" {
                1000
            } else {
                1
            };
            let ordinal = id / stride;
            if id % stride != 0 || ordinal >= self.seen.len() || self.seen[ordinal] {
                return Err("duplicate/out-of-range fixture ID");
            }
            if columns == 3 {
                let url = if self.workload == "selective" {
                    b"evil.test/powershell".as_slice()
                } else if self.workload == "wide1k" || id % 10000 != 0 {
                    b"benign.test/normal".as_slice()
                } else {
                    b"evil.test/powershell".as_slice()
                };
                if bytes(batch.column(1).as_ref(), row)? != url {
                    return Err("URL value differs");
                }
                let extra = bytes(batch.column(2).as_ref(), row)?;
                let length = if self.workload == "wide1k" { 1024 } else { 16 };
                if extra.len() != length || extra.iter().any(|b| *b != b'x') {
                    return Err("extra value differs");
                }
            }
            self.seen[ordinal] = true;
            self.count += 1;
        }
        Ok(())
    }
    fn finish(&self) -> Result<(), &'static str> {
        if self.count != self.seen.len() {
            Err("missing fixture IDs")
        } else {
            Ok(())
        }
    }
}
fn block_size(cmd: &Value) -> Result<u64, &'static str> {
    match cmd.get("block_size") {
        None => Ok(1024),
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=16384).contains(n))
            .ok_or("block size must be an integer between 1 and 16384"),
    }
}
async fn run(client: &AntalyaClient, cmd: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let block_size = block_size(cmd)?;
    let sql = cmd["sql"].as_str().ok_or("SQL")?;
    if sql.len() > 16384 {
        return Err("SQL bound".into());
    }
    let expected = usize::try_from(cmd["rows"].as_u64().ok_or("rows")?)?;
    let verify = cmd["verify"].as_bool().ok_or("verification mode")?;
    let mut oracle = ValueOracle::new(cmd["workload"].as_str().ok_or("workload")?, expected)?;
    let mut query = Query::new(sql, format!("fdw-warm-{}", uuid::Uuid::new_v4()))?;
    for (k, v) in [
        ("max_threads", "2"),
        ("max_block_size", "1024"),
        ("max_result_rows", "1000000"),
        ("max_result_bytes", "67108864"),
        ("result_overflow_mode", "throw"),
        ("max_memory_usage", "4000000000"),
        ("max_execution_time", "60"),
        ("use_query_cache", "0"),
        ("use_query_condition_cache", "0"),
        ("output_format_arrow_string_as_string", "0"),
        ("output_format_arrow_low_cardinality_as_dictionary", "0"),
        ("session_timezone", "UTC"),
        ("cancel_http_readonly_queries_on_client_close", "1"),
    ] {
        query = query.setting(k, v)?;
    }
    query = query.setting("max_block_size", block_size.to_string())?;
    let start = Instant::now();
    let mut execution = client.fetch_arrow(query).await?;
    let (mut rows, mut peak, mut charged) = (0usize, 0usize, 0usize);
    while let Some(batch) = execution.next_batch().await? {
        rows = rows.checked_add(batch.num_rows()).ok_or("row overflow")?;
        if rows > expected {
            return Err("extra rows".into());
        }
        peak = peak.max(batch.charged_bytes());
        charged = charged.saturating_add(batch.charged_bytes());
        if verify {
            oracle.check(&batch)?;
        }
    }
    let elapsed = start.elapsed().as_secs_f64() * 1000.;
    if execution.outcome() != Outcome::Succeeded || rows != expected {
        return Err("incomplete execution".into());
    }
    if verify {
        oracle.finish()?;
    }
    let m = execution.metrics();
    let ms = |d: Duration| d.as_secs_f64() * 1000.;
    Ok(
        json!({"route":"direct_antalya_rs","query_id":execution.query_id(),"rows":rows,"block_size":block_size,
        "request_ms":elapsed,"value_control":verify,"peak_individual_batch_bytes":peak,
        "charged_batch_bytes":charged,"first_batch_ms":m.first_batch.map(ms),"http_body_wait_ms":m.transfer.map(ms),
        "arrow_decode_ms":m.decode.map(ms),"framing_decode_ms":ms(m.framing),"terminal_ms":ms(m.terminal),
        "data_poll_ms":ms(m.data_poll),"http_body_bytes":m.wire_bytes,"control_requests":m.control_requests,
        "flush_ms":ms(m.flush_time),"observe_ms":ms(m.observe_time),"poll_wait_ms":ms(m.poll_wait),
        "flush_attempts":m.flush_attempts,"flush_failures":m.flush_failures,"polls":m.polls,
        "poll_sleeps":m.poll_sleeps,"stream_end_ms":m.stream_end.map(ms),
        "identity_ms":ms(m.identity),"queued_ms":ms(m.queued),"total_ms":ms(m.total)}),
    )
}
fn failure(error: &(dyn std::error::Error + 'static)) -> Value {
    let mut value =
        json!({"error":"direct FDW run failed; inspect private worker and correlated native logs"});
    if let Some(error) = error.downcast_ref::<antalya_rs::Error>() {
        value["error_kind"] = json!(format!("{:?}", error.kind()));
        if let Some(phase) = error.phase() {
            value["error_phase"] = json!(format!("{phase:?}"));
        }
    }
    value
}
#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let args: Vec<_> = std::env::args().collect();
    let strategy = match args.get(1).map(String::as_str).unwrap_or("flush_first") {
        "flush_first" => TerminalStrategy::FlushFirst,
        "observe_first" => TerminalStrategy::ObserveFirst,
        "observe_only" => TerminalStrategy::ObserveOnly,
        "in_band" => TerminalStrategy::InBand,
        _ => return Err("invalid policy".into()),
    };
    let batch_bytes = args
        .get(2)
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(16 << 20);
    if !(1 << 20..=32 << 20).contains(&batch_bytes) {
        return Err("batch bound".into());
    }
    let client = AntalyaClient::new(
        Config::new(
            Endpoint::parse(&std::env::var("ANTALYA_TEST_URL")?)?,
            Credentials::bearer(&std::env::var("ANTALYA_TEST_TOKEN")?)?,
        )
        .timeout(Duration::from_secs(65))
        .terminal_policy(TerminalPolicy {
            strategy,
            ..Default::default()
        })
        .execution_limits(ExecutionLimits {
            max_batch_bytes: batch_bytes,
            ..Default::default()
        }),
    )?;
    println!(
        "{}",
        json!({"ready":true,"client":"antalya-rs 0.1.0","client_setup_ms":start.elapsed().as_secs_f64()*1000.})
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
            let n = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |n| n + 1);
            if bytes.len() + n > 65536 {
                return Err("command bound".into());
            }
            let done = available[n - 1] == b'\n';
            bytes.extend_from_slice(&available[..n]);
            input.consume(n);
            if done {
                break;
            }
        }
        if bytes.is_empty() {
            break;
        }
        let cmd: Value = serde_json::from_slice(&bytes)?;
        let result = match tokio::time::timeout(Duration::from_secs(65), run(&client, &cmd)).await {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                // The harness redirects stderr to a private mode-0600 log.
                // Keep detailed transport errors out of the public protocol.
                eprintln!("direct FDW run error: {error:?}");
                failure(error.as_ref())
            }
            Err(_) => json!({"error":"direct FDW worker exceeded its 65-second deadline"}),
        };
        println!("{result}");
        io::stdout().flush()?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, BinaryArray, Int64Array, RecordBatch};
    use std::sync::Arc;
    #[test]
    fn resource_refusals_remain_typed_without_exposing_other_error_text() {
        let limit =
            antalya_rs::Error::at(antalya_rs::Phase::Data, antalya_rs::Cause::LimitExceeded);
        let value = failure(&limit);
        assert_eq!(value["error_kind"], "LimitExceeded");
        assert_eq!(value["error_phase"], "Data");
        let sensitive = std::io::Error::other("private-url-and-token");
        let value = failure(&sensitive);
        assert!(!value.to_string().contains("private-url"));
        assert!(value.get("error_kind").is_none());
        assert_ne!(
            failure(&antalya_rs::Error::Unconfirmed)["error_kind"],
            "LimitExceeded"
        );
    }
    #[test]
    fn block_size_is_bounded_and_defaults_to_historical_setting() {
        assert_eq!(block_size(&json!({})).unwrap(), 1024);
        for n in [1, 1024, 4096, 16384] {
            assert_eq!(block_size(&json!({"block_size": n})).unwrap(), n);
        }
        for value in [
            json!(0),
            json!(16385),
            json!(-1),
            json!(1.5),
            json!("4096"),
            json!(null),
        ] {
            assert!(block_size(&json!({"block_size": value})).is_err());
        }
    }
    #[test]
    fn selective_control_checks_sparse_ids_and_values() {
        let mut oracle = ValueOracle::new("selective", 3).unwrap();
        oracle
            .check(&batch(vec![2000, 0], vec![b"evil.test/powershell"; 2]))
            .unwrap();
        assert!(oracle.finish().is_err());
        oracle
            .check(&batch(vec![1000], vec![b"evil.test/powershell"]))
            .unwrap();
        oracle.finish().unwrap();
        for id in [1, 999, 3000, -1000] {
            assert!(
                ValueOracle::new("selective", 3)
                    .unwrap()
                    .check(&batch(vec![id], vec![b"evil.test/powershell"]))
                    .is_err()
            );
        }
        assert!(ValueOracle::new("selective", 1001).is_err());
    }
    fn batch(ids: Vec<i64>, urls: Vec<&[u8]>) -> RecordBatch {
        let n = ids.len();
        RecordBatch::try_from_iter(vec![
            ("id", Arc::new(Int64Array::from(ids)) as ArrayRef),
            ("url", Arc::new(BinaryArray::from(urls)) as ArrayRef),
            (
                "extra",
                Arc::new(BinaryArray::from(vec![b"xxxxxxxxxxxxxxxx".as_slice(); n])) as ArrayRef,
            ),
        ])
        .unwrap()
    }
    #[test]
    fn exact_control_checks_fields_and_id_multiset_across_batches() {
        let mut oracle = ValueOracle::new("smalltext", 3).unwrap();
        oracle
            .check(&batch(
                vec![2, 0],
                vec![b"benign.test/normal", b"evil.test/powershell"],
            ))
            .unwrap();
        assert!(oracle.finish().is_err());
        oracle
            .check(&batch(vec![1], vec![b"benign.test/normal"]))
            .unwrap();
        oracle.finish().unwrap();
        assert!(
            oracle
                .check(&batch(vec![1], vec![b"benign.test/normal"]))
                .is_err()
        );
        let mut oracle = ValueOracle::new("smalltext", 1).unwrap();
        assert!(oracle.check(&batch(vec![0], vec![b"bad"])).is_err());
    }
    #[test]
    fn bounds_and_nulls_are_rejected() {
        assert!(ValueOracle::new("narrow", 1000001).is_err());
        assert!(ValueOracle::new("other", 1).is_err());
        let bad = RecordBatch::try_from_iter(vec![(
            "id",
            Arc::new(Int64Array::from(vec![None])) as ArrayRef,
        )])
        .unwrap();
        assert!(ValueOracle::new("narrow", 1).unwrap().check(&bad).is_err());
    }
}
