// SPDX-License-Identifier: Apache-2.0
//! Direct published-client counterpart to tests/benchmarks/fdw_profile.py.
use antalya_rs::{
    AntalyaClient, Config, Credentials, Endpoint, ExecutionLimits, Query, TerminalPolicy,
    TerminalStrategy,
};
use serde_json::{Value, json};
use std::{
    io::{self, Read},
    time::{Duration, Instant},
};
#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().lock().take(65537).read_to_string(&mut input)?;
    if input.len() > 65536 {
        return Err("command too large".into());
    }
    let cmd: Value = serde_json::from_str(&input)?;
    let strategy = match cmd["policy"].as_str().ok_or("policy")? {
        "flush_first" => TerminalStrategy::FlushFirst,
        "observe_first" => TerminalStrategy::ObserveFirst,
        "observe_only" => TerminalStrategy::ObserveOnly,
        "in_band" => TerminalStrategy::InBand,
        _ => return Err("policy".into()),
    };
    let client = AntalyaClient::new(
        Config::new(
            Endpoint::parse(&std::env::var("ANTALYA_TEST_URL")?)?,
            Credentials::bearer(&std::env::var("ANTALYA_TEST_TOKEN")?)?,
        )
        .timeout(Duration::from_secs(300))
        .terminal_policy(TerminalPolicy {
            strategy,
            ..Default::default()
        })
        .execution_limits(ExecutionLimits {
            max_batch_bytes: cmd["batch_bytes"].as_u64().ok_or("batch_bytes")? as usize,
            ..Default::default()
        }),
    )?;
    let mut query = Query::new(
        cmd["sql"].as_str().ok_or("sql")?,
        format!("direct-profile-{}", uuid::Uuid::new_v4()),
    )?;
    for (k, v) in [
        ("max_block_size", "1024"),
        ("max_threads", "2"),
        ("max_result_rows", "1000000"),
        ("max_result_bytes", "67108864"),
        ("result_overflow_mode", "throw"),
        ("output_format_arrow_string_as_string", "0"),
        ("output_format_arrow_low_cardinality_as_dictionary", "0"),
        ("session_timezone", "UTC"),
        ("cancel_http_readonly_queries_on_client_close", "1"),
    ] {
        query = query.setting(k, v)?;
    }
    let start = Instant::now();
    let mut execution = client.fetch_arrow(query).await?;
    let mut rows = 0;
    let mut peak = 0;
    let mut charged = 0;
    while let Some(batch) = execution.next_batch().await? {
        rows += batch.num_rows();
        peak = peak.max(batch.charged_bytes());
        charged += batch.charged_bytes();
    }
    let elapsed = start.elapsed().as_secs_f64() * 1000.;
    let m = execution.metrics();
    let ms = |d: Duration| d.as_secs_f64() * 1000.;
    println!(
        "{}",
        json!({"route":"direct_antalya_rs","query_id":execution.query_id(),"rows":rows,"peak_individual_batch_bytes":peak,"charged_batch_bytes":charged,"wall_ms":elapsed,"first_batch_ms":m.first_batch.map(ms),"http_body_wait_ms":m.transfer.map(ms),"arrow_decode_ms":m.decode.map(ms),"framing_decode_ms":ms(m.framing),"terminal_ms":ms(m.terminal),"data_poll_ms":ms(m.data_poll),"http_body_bytes":m.wire_bytes,"control_requests":m.control_requests,"flush_ms":ms(m.flush_time),"observe_ms":ms(m.observe_time),"poll_wait_ms":ms(m.poll_wait)})
    );
    Ok(())
}
