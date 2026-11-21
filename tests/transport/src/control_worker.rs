// SPDX-License-Identifier: Apache-2.0
//! Benchmark-only sequential cohorts; no result is published before terminal proof.
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
use antalya_rs::{Credentials, Endpoint, Format, Query};
use serde_json::{Value, json};
use std::io::{self, BufRead, Read, Write};
use std::time::{Duration, Instant};
use wildcard_transport_format::{Bench, Decoded, Limits, Scenario, consume};

#[cfg(test)]
mod tests {
    use super::*;
    fn pending(id: &str) -> Pending {
        Pending {
            id: id.into(),
            marker: format!("marker-{id}"),
            start: Instant::now(),
            data_ms: 1.0,
            digest: String::new(),
            bytes: 1,
        }
    }
    fn row(id: &str) -> Value {
        json!({"query_id":id,"log_comment":format!("marker-{id}"),"user":"fixture",
            "starts":1,"finishes":1,"errors":0,"result_rows":1,"events":2})
    }
    fn proof(rows: Vec<Value>) -> Value {
        json!({"rows":rows.len(),"data":rows})
    }
    #[test]
    fn exact_cohort_is_required() {
        let requests = [pending("a"), pending("b")];
        assert!(validate_terminal(&proof(vec![row("b"), row("a")]), &requests, "fixture").is_ok());
        for rows in [
            vec![],
            vec![row("a")],
            vec![row("a"), row("a")],
            vec![row("a"), row("wrong")],
        ] {
            assert!(validate_terminal(&proof(rows), &requests, "fixture").is_err());
        }
    }
    #[test]
    fn one_late_failure_blocks_the_entire_cohort() {
        let requests = [pending("a"), pending("b")];
        for (key, value) in [
            ("errors", json!(1)),
            ("finishes", json!(0)),
            ("starts", json!(2)),
            ("result_rows", json!(2)),
            ("events", json!(3)),
            ("user", json!("other")),
            ("log_comment", json!("other")),
            ("user", Value::Null),
        ] {
            let mut bad = row("b");
            bad[key] = value;
            assert!(validate_terminal(&proof(vec![row("a"), bad]), &requests, "fixture").is_err());
        }
        let mut bad = proof(vec![row("a"), row("b")]);
        bad["exception"] = json!("late");
        assert!(validate_terminal(&bad, &requests, "fixture").is_err());
    }
    #[test]
    fn missing_or_wrong_identity_refuses() {
        for identity in [
            json!({}),
            json!({"version":VERSION}),
            json!({"version":VERSION,"principal":""}),
            json!({"version":"other","principal":"fixture"}),
            json!({"version":VERSION,"principal":"other"}),
        ] {
            assert!(validate_identity(&identity).is_err());
        }
        assert!(validate_identity(&json!({"version":VERSION,"principal":"fixture"})).is_ok());
    }
    #[test]
    fn data_is_exact_and_late_errors_refuse() {
        let valid =
            json!({"rows":1,"meta":[{"name":"id","type":"UInt64"}],"data":[{"id":"32768"}]});
        assert!(validate_data(&valid).is_ok());
        for (key, value) in [
            ("exception", json!("late")),
            ("rows", json!(2)),
            ("data", json!([{ "id":1 }])),
        ] {
            let mut bad = valid.clone();
            bad[key] = value;
            assert!(validate_data(&bad).is_err());
        }
    }
}

const VERSION: &str = "26.6.8.20004.altinityantalya";
const CAP: usize = 1024 * 1024;
const COHORT_DEADLINE: Duration = Duration::from_secs(30);
#[derive(Debug)]
enum Error {
    Configuration,
    Command,
    Limit,
    Unconfirmed,
    InvalidData,
    Deadline,
    Core(antalya_rs::Error),
    Benchmark(wildcard_transport_format::Error),
    Http(reqwest::Error),
    Io(io::Error),
    Json(serde_json::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Configuration => "configuration",
            Self::Command => "command",
            Self::Limit => "limit",
            Self::Unconfirmed => "unconfirmed",
            Self::InvalidData => "invalid data",
            Self::Deadline => "cohort deadline",
            Self::Core(_) => "core",
            Self::Benchmark(_) => "benchmark",
            Self::Http(_) => "http",
            Self::Io(_) => "io",
            Self::Json(_) => "json",
        })
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(e) => Some(e),
            Self::Benchmark(e) => Some(e),
            Self::Http(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}
macro_rules! from_error {
    ($t:ty,$v:ident) => {
        impl From<$t> for Error {
            fn from(e: $t) -> Self {
                Self::$v(e)
            }
        }
    };
}
from_error!(antalya_rs::Error, Core);
from_error!(wildcard_transport_format::Error, Benchmark);
from_error!(reqwest::Error, Http);
from_error!(io::Error, Io);
from_error!(serde_json::Error, Json);
type Result<T> = std::result::Result<T, Error>;
fn number(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}
fn fresh() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
fn validate_identity(value: &Value) -> Result<()> {
    if value["version"] != VERSION || value["principal"] != "fixture" {
        return Err(Error::Configuration);
    }
    Ok(())
}
fn envelope(value: &Value) -> Result<&Vec<Value>> {
    if value.get("exception").is_some() || value.get("error").is_some() {
        return Err(Error::Unconfirmed);
    }
    let rows = value["data"].as_array().ok_or(Error::InvalidData)?;
    if rows.len() > 4 || number(&value["rows"]) != Some(rows.len() as u64) {
        return Err(Error::InvalidData);
    }
    Ok(rows)
}
fn validate_data(value: &Value) -> Result<Decoded> {
    let rows = envelope(value)?;
    if rows.len() != 1 || value["meta"] != json!([{"name":"id","type":"UInt64"}]) {
        return Err(Error::InvalidData);
    }
    let row = rows.first().ok_or(Error::InvalidData)?;
    if row.as_object().map(|r| r.len()) != Some(1) || number(&row["id"]) != Some(32768) {
        return Err(Error::InvalidData);
    }
    Ok(Decoded::Json(vec![(32768, None)]))
}
struct Pending {
    id: String,
    marker: String,
    start: Instant,
    data_ms: f64,
    digest: String,
    bytes: usize,
}
fn validate_terminal<'a>(
    value: &'a Value,
    requests: &[Pending],
    principal: &str,
) -> Result<Vec<&'a Value>> {
    let rows = envelope(value)?;
    if requests.is_empty() || requests.len() > 4 || rows.len() != requests.len() {
        return Err(Error::Unconfirmed);
    }
    let mut verified = Vec::with_capacity(requests.len());
    for (index, request) in requests.iter().enumerate() {
        if requests[..index]
            .iter()
            .any(|p| p.id == request.id || p.marker == request.marker)
        {
            return Err(Error::Unconfirmed);
        }
        let mut matching = rows.iter().filter(|r| r["query_id"] == request.id);
        let row = matching.next().ok_or(Error::Unconfirmed)?;
        if matching.next().is_some()
            || row["log_comment"] != request.marker
            || row["user"] != principal
            || number(&row["starts"]) != Some(1)
            || number(&row["finishes"]) != Some(1)
            || number(&row["errors"]) != Some(0)
            || number(&row["events"]) != Some(2)
            || number(&row["result_rows"]) != Some(1)
        {
            return Err(Error::Unconfirmed);
        }
        verified.push(row);
    }
    Ok(verified)
}
struct Worker {
    client: reqwest::Client,
    endpoint: Endpoint,
    credentials: Credentials,
}
impl Worker {
    fn request(&self, query: &Query, format: Option<Format>) -> Result<reqwest::Request> {
        let mut req = match format {
            Some(f) => query.build_request(&self.client, &self.endpoint, &self.credentials, f)?,
            None => query.build_command_request(&self.client, &self.endpoint, &self.credentials)?,
        };
        req.headers_mut().insert(
            "accept-encoding",
            reqwest::header::HeaderValue::from_static("identity"),
        );
        req.url_mut().query_pairs_mut().extend_pairs([
            ("max_threads", "1"),
            ("max_execution_time", "5"),
            ("max_memory_usage", "200000000"),
            ("max_result_rows", "16"),
            ("max_result_bytes", "1048576"),
            ("result_overflow_mode", "throw"),
            ("use_query_cache", "0"),
        ]);
        Ok(req)
    }
    async fn download(&self, request: reqwest::Request, id: &str) -> Result<Vec<u8>> {
        let mut response = self.client.execute(request).await?;
        if response.status() != 200
            || response
                .headers()
                .get("x-clickhouse-query-id")
                .and_then(|v| v.to_str().ok())
                != Some(id)
            || response
                .headers()
                .get("content-encoding")
                .is_some_and(|v| v != "identity")
        {
            return Err(Error::Unconfirmed);
        }
        if response.content_length().is_some_and(|n| n > CAP as u64) {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > CAP.saturating_sub(bytes.len()) {
                return Err(Error::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn data(&self, scenario: &Scenario) -> Result<Pending> {
        let start = Instant::now();
        let id = fresh();
        let marker = fresh();
        let query = scenario.query(&id)?;
        let mut req = self.request(&query, Some(Format::Json))?;
        req.url_mut().query_pairs_mut().extend_pairs([
            ("log_comment", marker.as_str()),
            ("log_queries", "1"),
            ("log_queries_probability", "1"),
            ("log_queries_min_query_duration_ms", "0"),
        ]);
        let bytes = self.download(req, &id).await?;
        let decoded = validate_data(&serde_json::from_slice(&bytes)?)?;
        let (_, digest) = consume(&decoded, true, Limits::default())?;
        Ok(Pending {
            id,
            marker,
            start,
            data_ms: ms(start),
            digest,
            bytes: bytes.len(),
        })
    }
    async fn prove(
        &self,
        requests: &[Pending],
        cohort_start: Instant,
    ) -> Result<(Vec<Value>, Value)> {
        let begin = Instant::now();
        let flush = Query::new("SYSTEM FLUSH LOGS", fresh())?;
        let bytes = self
            .download(self.request(&flush, None)?, flush.query_id())
            .await?;
        if !bytes.iter().all(u8::is_ascii_whitespace) {
            return Err(Error::Unconfirmed);
        }
        let flush_ms = ms(begin);
        let observe = Instant::now();
        let terms = (0..requests.len())
            .map(|i| format!("{{id{i}:String}}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT query_id,log_comment,user,count() AS events,countIf(type='QueryStart') AS starts,countIf(type='QueryFinish') AS finishes,countIf(type IN ('ExceptionBeforeStart','ExceptionWhileProcessing')) AS errors,max(result_rows) AS result_rows,max(query_duration_ms) AS query_duration_ms FROM system.query_log WHERE query_id IN ({terms}) AND is_initial_query=1 GROUP BY query_id,log_comment,user"
        );
        let mut query = Query::new(sql, fresh())?;
        for (i, request) in requests.iter().enumerate() {
            query = query.parameter(&format!("id{i}"), &request.id)?;
        }
        let bytes = self
            .download(self.request(&query, Some(Format::Json))?, query.query_id())
            .await?;
        let value: Value = serde_json::from_slice(&bytes)?;
        let rows = validate_terminal(&value, requests, "fixture")?;
        let publication = Instant::now();
        let samples = requests
            .iter()
            .zip(rows)
            .map(|(request, row)| {
                json!({
            "query_id":request.id,"execution_marker":request.marker,"rows":1,"bytes":request.bytes,
            "result_digest":request.digest,"data_validated_ms":request.data_ms,
            "publication_latency_ms":publication.duration_since(request.start).as_secs_f64()*1000.0,
            "request_start_offset_ms":request.start.duration_since(cohort_start).as_secs_f64()*1000.0,
            "proof_gate_offset_ms":publication.duration_since(cohort_start).as_secs_f64()*1000.0,
            "terminal":row,"completion":"Succeeded"})
            })
            .collect();
        Ok((
            samples,
            json!({"flush_ms":flush_ms,"observe_and_validate_ms":ms(observe),
            "flush_query_id":flush.query_id(),"observe_query_id":query.query_id(),"queries":requests.len()}),
        ))
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        let _ = writeln!(io::stderr().lock(), "control worker failed: {error}");
        std::process::exit(1)
    }
}
async fn run() -> Result<()> {
    let url = std::env::var("ANTALYA_TEST_URL").map_err(|_| Error::Configuration)?;
    let token = std::env::var("ANTALYA_TEST_TOKEN").map_err(|_| Error::Configuration)?;
    let bench = Bench::new(&url, &token, Limits::default())?;
    let identity = bench.identity().await?;
    validate_identity(&identity)?;
    let worker = Worker {
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .http1_only()
            .pool_max_idle_per_host(1)
            .timeout(Duration::from_secs(10))
            .build()?,
        endpoint: Endpoint::parse(&url)?,
        credentials: Credentials::bearer(&token)?,
    };
    let scenario = Scenario::all()
        .into_iter()
        .find(|s| s.count)
        .ok_or(Error::Configuration)?;
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
        let count = command["cohort_size"]
            .as_u64()
            .filter(|n| [1, 4].contains(n))
            .ok_or(Error::Command)?;
        let mode = command["mode"]
            .as_str()
            .filter(|s| ["individual", "cohort"].contains(s))
            .ok_or(Error::Command)?;
        let start = Instant::now();
        let (samples, controls) = tokio::time::timeout(COHORT_DEADLINE, async {
            let mut pending = Vec::with_capacity(count as usize);
            let mut samples = Vec::new();
            let mut controls = Vec::new();
            for _ in 0..count {
                pending.push(worker.data(&scenario).await?);
                if mode == "individual" {
                    let (verified, control) = worker.prove(&pending, start).await?;
                    samples.extend(verified);
                    controls.push(control);
                    pending.clear();
                }
            }
            if mode == "cohort" {
                let (verified, control) = worker.prove(&pending, start).await?;
                samples.extend(verified);
                controls.push(control);
            }
            Ok::<_, Error>((samples, controls))
        })
        .await
        .map_err(|_| Error::Deadline)??;
        if start.elapsed() >= COHORT_DEADLINE {
            return Err(Error::Deadline);
        }
        let wall_ms = ms(start);
        let report = json!({"mode":mode,"cohort_size":count,"cohort_wall_ms":wall_ms,"cohort_queries_per_second":count as f64*1000.0/wall_ms,
            "samples":samples,"controls":controls,"identity":identity,"concurrency":1,"cohort_deadline_ms":30000,
            "publication_note":"each latency ends at its proof gate; cohort report emitted only after all gates succeed"});
        let serialized = serde_json::to_vec(&report)?;
        if start.elapsed() >= COHORT_DEADLINE {
            return Err(Error::Deadline);
        }
        io::stdout().write_all(&serialized)?;
        io::stdout().write_all(b"\n")?;
        io::stdout().flush()?;
    }
    Ok(())
}
