// SPDX-License-Identifier: Apache-2.0
//! Both engines use this one Rust HTTP pool. Orchestration is untimed.
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
mod budget_refusal;
mod elastic_target;
mod measurement;
#[cfg(feature = "production")]
mod shared_engine;
mod worker_parameters;
use measurement::{CLOCK_BOUNDARY, finish_verified};

use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{self, BufRead, Read, Write};
use std::time::{Duration, Instant};

const CAP: usize = 16 * 1024 * 1024;
const COMMAND_CAP: usize = 128 * 1024;
#[derive(Debug)]
enum Error {
    Configuration,
    #[cfg(feature = "production")]
    Shared(antalya_rs::Error),
    Input,
    Transport,
    Status,
    Limit,
    Json,
    Incomplete,
    Values,
    Io,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        #[cfg(feature = "production")]
        if let Self::Shared(error) = self {
            return Some(error);
        }
        None
    }
}
type Result<T> = std::result::Result<T, Error>;

fn uint(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .ok_or(Error::Values)
}
fn checked_digest(
    value: &Value,
    engine: &str,
    mode: &str,
    expected: u64,
) -> Result<(usize, String)> {
    checked_digest_field(value, engine, mode, expected, "v")
}
fn checked_digest_field(
    value: &Value,
    engine: &str,
    mode: &str,
    expected: u64,
    value_field: &str,
) -> Result<(usize, String)> {
    if value.get("error").is_some() || value.get("exception").is_some() {
        return Err(Error::Incomplete);
    }
    let mut rows = Vec::new();
    if engine == "elastic" {
        let shards = &value["_shards"];
        if value["timed_out"] != false
            || value.get("terminated_early").is_some_and(|v| v != false)
            || uint(&shards["total"])? == 0
            || uint(&shards["failed"])? != 0
            || uint(&shards["successful"])? != uint(&shards["total"])?
        {
            return Err(Error::Incomplete);
        }
        let hits = value["hits"]["hits"].as_array().ok_or(Error::Values)?;
        if hits.len() > 32768 {
            return Err(Error::Limit);
        }
        if mode == "count" {
            if value["hits"]["total"]["relation"] != "eq"
                || uint(&value["hits"]["total"]["value"])? != expected
            {
                return Err(Error::Incomplete);
            }
            if !hits.is_empty() {
                return Err(Error::Values);
            }
            rows.push((expected, None));
        } else {
            for hit in hits {
                if hit["_source"].as_object().map(|source| source.len()) != Some(2) {
                    return Err(Error::Values);
                }
                rows.push((
                    uint(&hit["_source"]["id"])?,
                    Some(hit["_source"][value_field].as_str().ok_or(Error::Values)?),
                ));
            }
        }
    } else {
        let data = value["data"].as_array().ok_or(Error::Values)?;
        if data.len() > 32768 {
            return Err(Error::Limit);
        }
        if uint(&value["rows"])? != data.len() as u64 {
            return Err(Error::Incomplete);
        }
        for row in data {
            if row.as_object().map(|r| r.len()) != Some(if mode == "count" { 1 } else { 2 }) {
                return Err(Error::Values);
            }
            rows.push((
                uint(&row["id"])?,
                if mode == "count" {
                    None
                } else {
                    Some(row["value"].as_str().ok_or(Error::Values)?)
                },
            ));
        }
    }
    if rows.len() > 32768 {
        return Err(Error::Limit);
    }
    if mode == "count" && (rows.len() != 1 || rows[0].0 != expected) {
        return Err(Error::Values);
    }
    let mut hash = Sha256::new();
    let mut previous = None;
    for (id, text) in &rows {
        if mode != "count" && previous.is_some_and(|p| p >= *id) {
            return Err(Error::Values);
        }
        previous = Some(*id);
        hash.update(id.to_le_bytes());
        if let Some(text) = text {
            if text.len() > 8192 {
                return Err(Error::Limit);
            }
            hash.update([1]);
            hash.update((text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        }
    }
    Ok((rows.len(), format!("{:x}", hash.finalize())))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        let _ = writeln!(io::stderr().lock(), "engine worker failed: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let antalya = std::env::var("ANTALYA_TEST_URL").map_err(|_| Error::Configuration)?;
    let elastic_url = std::env::var("ELASTIC_TEST_URL").map_err(|_| Error::Configuration)?;
    let elastic = elastic_target::Target::new(
        &elastic_url,
        elastic_target::optional_env("ELASTIC_TEST_AUTH")?.as_deref(),
        elastic_target::optional_env("ELASTIC_TEST_INDEX")?.as_deref(),
        elastic_target::optional_env("ELASTIC_TEST_VALUE_FIELD")?.as_deref(),
    )?;
    let token = std::env::var("ANTALYA_TEST_TOKEN").map_err(|_| Error::Configuration)?;
    let mut auth =
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| Error::Configuration)?;
    auth.set_sensitive(true);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .http1_only()
        .pool_max_idle_per_host(1)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| Error::Configuration)?;
    #[cfg(feature = "production")]
    let shared = shared_engine::client(&antalya, &token)?;
    #[cfg(feature = "production")]
    let framed =
        shared_engine::client_with_policy(&antalya, &token, antalya_rs::TerminalStrategy::InBand)?;
    let mut input = io::stdin().lock();
    loop {
        let mut command = Vec::new();
        let size = (&mut input)
            .take((COMMAND_CAP + 1) as u64)
            .read_until(b'\n', &mut command)
            .map_err(|_| Error::Io)?;
        if size == 0 {
            break;
        }
        if size > COMMAND_CAP || command.last() != Some(&b'\n') {
            return Err(Error::Limit);
        }
        let command: Value = serde_json::from_slice(&command).map_err(|_| Error::Input)?;
        let engine = command["engine"].as_str().ok_or(Error::Input)?;
        let mode = command["mode"]
            .as_str()
            .filter(|s| ["count", "page", "bulk", "explain"].contains(s))
            .ok_or(Error::Input)?;
        #[cfg(feature = "production")]
        if matches!(engine, "antalya_shared" | "antalya_shared_optimistic") {
            let selected = if shared_engine::policy(&command)? == "in_band" {
                &framed
            } else {
                &shared
            };
            let sample = if mode == "explain" {
                shared_engine::explain(selected, &command).await?
            } else {
                shared_engine::measure(selected, &command).await?
            };
            write_sample(&sample)?;
            continue;
        }
        if mode == "explain" {
            return Err(Error::Input);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let start = Instant::now();
        let request = if engine == "antalya" {
            let sql = command["sql"].as_str().ok_or(Error::Input)?;
            let settings = command["settings"].as_object().ok_or(Error::Input)?;
            let mut params: Vec<(String, String)> = settings
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string()),
                    )
                })
                .collect();
            params.extend(
                worker_parameters::scope_parameters(&command)?
                    .into_iter()
                    .map(|(key, value)| (format!("param_{key}"), value)),
            );
            params.extend([
                ("query_id".into(), id.clone()),
                ("log_comment".into(), id.clone()),
                ("log_queries".into(), "1".into()),
                ("log_queries_probability".into(), "1".into()),
                ("log_queries_min_query_duration_ms".into(), "0".into()),
            ]);
            let mut url = reqwest::Url::parse(&antalya).map_err(|_| Error::Configuration)?;
            url.query_pairs_mut().extend_pairs(&params);
            client
                .post(url)
                .header(AUTHORIZATION, auth.clone())
                .body(format!("{sql} FORMAT JSON"))
        } else if engine == "elastic" {
            elastic.request(&client, &command["body"], mode)?
        } else {
            return Err(Error::Input);
        };
        let mut response = request
            .header("accept-encoding", "identity")
            .send()
            .await
            .map_err(|_| Error::Transport)?;
        let headers_ms = start.elapsed().as_secs_f64() * 1000.0;
        let status = response.status().as_u16();
        if status != 200 && !(engine == "elastic" && status == 429) {
            return Err(Error::Status);
        }
        if engine == "antalya"
            && response
                .headers()
                .get("x-clickhouse-query-id")
                .and_then(|v| v.to_str().ok())
                != Some(id.as_str())
        {
            return Err(Error::Incomplete);
        }
        if response.content_length().is_some_and(|n| n > CAP as u64) {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Transport)? {
            if chunk.len() > CAP.saturating_sub(bytes.len()) {
                return Err(Error::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        let request_body_ms = start.elapsed().as_secs_f64() * 1000.0;
        let decode_start = Instant::now();
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
        let decode_ms = decode_start.elapsed().as_secs_f64() * 1000.0;
        if engine == "elastic"
            && let Some(refusal) = budget_refusal::elastic(status, &value)
        {
            let mut sample = refusal.sample(&id, start.elapsed());
            sample["elastic_authentication"] = json!(elastic.authentication());
            write_sample(&sample)?;
            continue;
        }
        if status != 200 {
            return Err(Error::Status);
        }
        let consume_start = Instant::now();
        let expected = uint(&command["expected_total"])?;
        let (rows, digest) = if engine == "elastic" {
            checked_digest_field(&value, engine, mode, expected, elastic.value_field())?
        } else {
            checked_digest(&value, engine, mode, expected)?
        };
        let mut consume_ms = 0.0;
        let total_ms = finish_verified(&command, rows, &digest, || {
            consume_ms = consume_start.elapsed().as_secs_f64() * 1000.0;
            start.elapsed()
        })?
        .as_secs_f64()
            * 1000.0;
        let report = json!({"query_id":id,"headers_ms":headers_ms,"request_body_ms":request_body_ms,
            "decode_wall_ms":decode_ms,"consumer_wall_ms":consume_ms,"client_ms":total_ms,
            "rows":rows,"bytes":bytes.len(),"result_digest":digest,
            "clock_boundary":CLOCK_BOUNDARY,"result_verified":true,
            "elastic_authentication":if engine=="elastic" {json!(elastic.authentication())} else {Value::Null},
            "elastic_took_ms":if engine=="elastic" {value["took"].clone()} else {Value::Null}});
        write_sample(&report)?;
    }
    Ok(())
}

fn write_sample(report: &Value) -> Result<()> {
    serde_json::to_writer(io::stdout().lock(), report).map_err(|_| Error::Io)?;
    io::stdout().write_all(b"\n").map_err(|_| Error::Io)?;
    io::stdout().flush().map_err(|_| Error::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_partial_late_and_duplicate_results() {
        let base = json!({"rows":2,"data":[{"id":1,"value":"a"},{"id":2,"value":"b"}]});
        assert!(checked_digest(&base, "antalya", "page", 2).is_ok());
        let mut bad = base.clone();
        bad["exception"] = json!("late");
        assert!(checked_digest(&bad, "antalya", "page", 2).is_err());
        let mut bad = base.clone();
        bad["rows"] = json!(1);
        assert!(checked_digest(&bad, "antalya", "page", 2).is_err());
        let mut bad = base;
        bad["data"][1]["id"] = json!(1);
        assert!(checked_digest(&bad, "antalya", "page", 2).is_err());
        let elastic = json!({"timed_out":true,"_shards":{"total":1,"successful":1,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"hits":[]}});
        assert!(checked_digest(&elastic, "elastic", "count", 0).is_err());
    }
}

#[cfg(test)]
mod measurement_contract_tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn mismatched_oracle_cannot_read_or_publish_success_clock() {
        for command in [
            json!({"expected_rows":2,"expected_digest":"ok"}),
            json!({"expected_rows":1,"expected_digest":"wrong"}),
        ] {
            let called = Cell::new(false);
            let result = finish_verified(&command, 1, "ok", || {
                called.set(true);
                Duration::from_secs(9)
            });
            assert!(result.is_err());
            assert!(
                !called.get(),
                "oracle failure must precede success clock read"
            );
        }
        let called = Cell::new(false);
        let duration = finish_verified(
            &json!({"expected_rows":1,"expected_digest":"ok"}),
            1,
            "ok",
            || {
                called.set(true);
                Duration::from_secs(9)
            },
        )
        .unwrap();
        assert!(called.get());
        assert_eq!(duration, Duration::from_secs(9));
    }
    #[test]
    fn elastic_extra_source_columns_are_not_equivalent_work() {
        let mut value = json!({"timed_out":false,"_shards":{"total":1,"successful":1,"failed":0},"hits":{"hits":[{"_source":{"id":1,"v":"raw"}}]}});
        assert!(checked_digest(&value, "elastic", "page", 1).is_ok());
        value["hits"]["hits"][0]["_source"]["message"] = json!("another copy of raw");
        assert!(
            checked_digest(&value, "elastic", "page", 1).is_err(),
            "extra source columns change network/decode work"
        );
    }
}
