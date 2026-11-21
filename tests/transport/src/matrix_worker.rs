// SPDX-License-Identifier: Apache-2.0
//! Persistent benchmark worker. Stdio orchestration is outside sample clocks.
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
use antalya_rs::{AntalyaClient, Config, Credentials, Endpoint, Format, Outcome};
use serde_json::{Value, json};
use std::io::{self, BufRead, Read, Write};
use std::time::{Duration, Instant};
use wildcard_transport_format::{Bench, Decoded, Limits, Scenario, consume};

#[derive(Debug)]
enum Error {
    Configuration,
    Command,
    Limit,
    Unconfirmed,
    Client(antalya_rs::Error),
    Benchmark(wildcard_transport_format::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Configuration => "configuration",
            Self::Command => "command",
            Self::Limit => "limit",
            Self::Unconfirmed => "unconfirmed",
            Self::Client(_) => "client",
            Self::Benchmark(_) => "benchmark",
            Self::Io(_) => "io",
            Self::Json(_) => "json",
        })
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(e) => Some(e),
            Self::Benchmark(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}
impl From<antalya_rs::Error> for Error {
    fn from(e: antalya_rs::Error) -> Self {
        Self::Client(e)
    }
}
impl From<wildcard_transport_format::Error> for Error {
    fn from(e: wildcard_transport_format::Error) -> Self {
        Self::Benchmark(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
fn retained(
    rows: usize,
    bytes: usize,
    next_rows: usize,
    next_bytes: usize,
) -> Result<(usize, usize), Error> {
    let rows = rows.checked_add(next_rows).ok_or(Error::Limit)?;
    let bytes = bytes.checked_add(next_bytes).ok_or(Error::Limit)?;
    if rows > 100000 || bytes > 16 * 1024 * 1024 {
        return Err(Error::Limit);
    }
    Ok((rows, bytes))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if run().await.is_err() {
        let _ = writeln!(
            io::stderr().lock(),
            "matrix worker failed (details redacted)"
        );
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Error> {
    let url = std::env::var("ANTALYA_TEST_URL").map_err(|_| Error::Configuration)?;
    let token = std::env::var("ANTALYA_TEST_TOKEN").map_err(|_| Error::Configuration)?;
    let bench = Bench::new(&url, &token, Limits::default())?;
    let identity = bench.identity().await?;
    if identity["version"] != "26.6.8.20004.altinityantalya" {
        return Err(Error::Configuration);
    }
    let client = AntalyaClient::new(
        Config::new(Endpoint::parse(&url)?, Credentials::bearer(&token)?)
            .timeout(Duration::from_secs(20)),
    )?;
    let mut input = io::stdin().lock();
    loop {
        let mut line = Vec::new();
        let size = (&mut input).take(1025).read_until(b'\n', &mut line)?;
        if size == 0 {
            break;
        }
        if size > 1024 || line.last() != Some(&b'\n') {
            return Err(Error::Limit);
        }
        let command: Value = serde_json::from_slice(&line)?;
        let scenarios = Scenario::all();
        let scenario = scenarios
            .iter()
            .find(|s| command["scenario"] == s.name)
            .ok_or(Error::Command)?;
        let variant = command["variant"].as_str().ok_or(Error::Command)?;
        let result = if variant == "B" || variant == "C" {
            let format = if variant == "B" {
                Format::Json
            } else {
                Format::ArrowStream
            };
            let (decoded, sample) = bench.measure(scenario, format).await?;
            bench.validate(scenario, &decoded)?;
            serde_json::to_value(sample)?
        } else if variant == "D" {
            let id = uuid::Uuid::new_v4().to_string();
            let mut query = scenario.query(&id)?;
            for (key, value) in [
                ("max_execution_time", "5"),
                ("max_result_rows", "100000"),
                ("max_result_bytes", "16777216"),
                ("result_overflow_mode", "throw"),
                ("max_threads", "1"),
                ("use_query_cache", "0"),
                ("output_format_arrow_string_as_string", "1"),
            ] {
                query = query.setting(key, value)?;
            }
            let start = Instant::now();
            let mut execution = client.fetch_arrow(query).await?;
            let mut batches = Vec::new();
            let (mut retained_rows, mut retained_bytes) = (0, 0);
            let mut first_batch_ms = None;
            let terminal_next_ms;
            loop {
                let next_start = Instant::now();
                match execution.next().await? {
                    Some(batch) => {
                        first_batch_ms.get_or_insert(start.elapsed().as_secs_f64() * 1000.0);
                        (retained_rows, retained_bytes) = retained(
                            retained_rows,
                            retained_bytes,
                            batch.num_rows(),
                            batch.get_array_memory_size(),
                        )?;
                        if batches.len() >= 1024 {
                            return Err(Error::Limit);
                        }
                        batches.push(batch);
                    }
                    None => {
                        terminal_next_ms = next_start.elapsed().as_secs_f64() * 1000.0;
                        break;
                    }
                }
            }
            if execution.outcome() != Outcome::Succeeded {
                return Err(Error::Unconfirmed);
            }
            let fetch_verified_ms = start.elapsed().as_secs_f64() * 1000.0;
            let decoded = Decoded::Arrow(batches);
            let consume_start = Instant::now();
            let (rows, digest) = consume(&decoded, scenario.count, Limits::default())?;
            let consumer_wall_ms = consume_start.elapsed().as_secs_f64() * 1000.0;
            let total_wall_ms = start.elapsed().as_secs_f64() * 1000.0;
            bench.validate(scenario, &decoded)?;
            json!({"query_id":id,"rows":rows,"result_digest":digest,
                "first_batch_ms":first_batch_ms,"fetch_verified_ms":fetch_verified_ms,
                "terminal_next_ms":terminal_next_ms,"consumer_wall_ms":consumer_wall_ms,
                "total_wall_ms":total_wall_ms,"completion":"Succeeded",
                "decode_wall_ms":null,"completion_wall_ms":null,
                "phase_note":"upstream fetch/decode overlap; final next includes EOF and completion, not pure control time; consumer runs after verified fetch"})
        } else {
            return Err(Error::Command);
        };
        serde_json::to_writer(io::stdout().lock(), &result)?;
        io::stdout().write_all(b"\n")?;
        io::stdout().flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cumulative_memory_rows_and_overflow_are_rejected() {
        assert!(retained(0, 0, 100000, 16 * 1024 * 1024).is_ok());
        assert!(matches!(retained(100000, 0, 1, 0), Err(Error::Limit)));
        assert!(matches!(
            retained(0, 16 * 1024 * 1024, 0, 1),
            Err(Error::Limit)
        ));
        assert!(matches!(retained(usize::MAX, 0, 1, 0), Err(Error::Limit)));
    }
}
