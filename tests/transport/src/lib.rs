// SPDX-License-Identifier: Apache-2.0
//! B/C benchmark: one reqwest pool; buffered download, then bounded decoding.
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
use antalya_rs::{Credentials, Endpoint, Format, Parameter, Query};
use arrow_array::{Array, RecordBatch, StringArray, UInt64Array};
use arrow_ipc::{MessageHeader, reader::StreamReader};
use arrow_schema::DataType;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Cursor,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Configuration(&'static str),
    Transport(&'static str),
    Limit(&'static str),
    Completion(&'static str),
    InvalidData(&'static str),
    Io(&'static str),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Configuration(m)
            | Self::Transport(m)
            | Self::Limit(m)
            | Self::Completion(m)
            | Self::InvalidData(m)
            | Self::Io(m) => m,
        };
        f.write_str(message)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_rows: usize,
    pub max_value_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_rows: 100_000,
            max_value_bytes: 8192,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Scenario {
    pub name: &'static str,
    pub source_rows: u64,
    pub rows: usize,
    pub count: bool,
}
impl Scenario {
    pub fn page(rows: usize) -> Result<Self> {
        if rows == 0 || rows > 100_000 {
            return Err(Error::Configuration("invalid scenario size"));
        };
        Ok(Self {
            name: "page",
            source_rows: rows as u64,
            rows,
            count: false,
        })
    }
    pub fn all() -> Vec<Self> {
        vec![
            Self {
                name: "count",
                source_rows: 32768,
                rows: 1,
                count: true,
            },
            Self {
                name: "page",
                source_rows: 32768,
                rows: 32,
                count: false,
            },
            Self {
                name: "bulk",
                source_rows: 32768,
                rows: 32768,
                count: false,
            },
        ]
    }
    pub fn query(&self, id: &str) -> Result<Query> {
        let sql = if self.count {
            "SELECT count() AS id FROM numbers({source:UInt64})"
        } else {
            r#"SELECT number AS id, multiIf(number % 5 = 0, NULL, number % 5 = 1, '', number % 5 = 2, 'héllo\n\0', number % 5 = 3, 'slash\\quote"', concat('row-', toString(number))) AS value FROM numbers({source:UInt64}) ORDER BY id LIMIT {limit:UInt64}"#
        };
        Query::new(sql, id)
            .map_err(|_| Error::Configuration("query construction"))?
            .parameter("source", Parameter::UInt64(self.source_rows))
            .map_err(|_| Error::Configuration("query construction"))?
            .parameter("limit", Parameter::UInt64(self.rows as u64))
            .map_err(|_| Error::Configuration("query construction"))
    }
    pub fn fingerprints(&self) -> Result<(String, String)> {
        let client = reqwest::Client::new();
        let request = self
            .query("fingerprint")?
            .build_request(
                &client,
                &Endpoint::parse("http://localhost/")
                    .map_err(|_| Error::Configuration("endpoint"))?,
                &Credentials::bearer("unused").map_err(|_| Error::Configuration("credential"))?,
                Format::Json,
            )
            .map_err(|_| Error::InvalidData("request"))?;
        let body = request
            .body()
            .and_then(|b| b.as_bytes())
            .ok_or(Error::InvalidData("body"))?;
        let sql = body
            .strip_suffix(b"\nFORMAT JSON")
            .ok_or(Error::InvalidData("format suffix"))?;
        Ok((
            hex(sql),
            hex(format!("source={};limit={}", self.source_rows, self.rows).as_bytes()),
        ))
    }
}
#[derive(Debug, PartialEq)]
struct Row {
    id: u64,
    value: Option<String>,
}
pub enum Decoded {
    Json(Vec<(u64, Option<String>)>),
    Arrow(Vec<RecordBatch>),
}
#[derive(Debug, Serialize)]
pub struct Control {
    pub kind: &'static str,
    pub query_id: String,
    pub wall_ms: f64,
    pub succeeded: bool,
}
#[derive(Debug, Serialize, Default)]
pub struct Engine {
    pub query_duration_ms: Option<u64>,
    pub read_rows: Option<u64>,
    pub read_bytes: Option<u64>,
    pub result_rows: Option<u64>,
    pub selected_marks: Option<u64>,
    pub selected_parts: Option<u64>,
    pub physical_io_bytes: Option<u64>,
    pub candidates: Option<u64>,
}
#[derive(Debug, Serialize)]
pub struct Sample {
    pub query_id: String,
    pub execution_marker: String,
    pub format: &'static str,
    pub headers_ms: f64,
    pub first_body_byte_ms: Option<f64>,
    pub body_after_headers_ms: f64,
    pub request_body_ms: f64,
    pub decode_wall_ms: f64,
    pub decode_thread_cpu_ms: Option<f64>,
    pub consumer_wall_ms: f64,
    pub consumer_thread_cpu_ms: Option<f64>,
    pub completion_wall_ms: f64,
    pub total_wall_ms: f64,
    pub bytes: usize,
    pub rows: usize,
    pub batches: Option<usize>,
    pub result_digest: String,
    pub http_version: String,
    pub fixture_connection: Option<String>,
    pub controls: Vec<Control>,
    pub engine: Engine,
    pub completion: &'static str,
}
struct Download {
    bytes: Vec<u8>,
    headers_ms: f64,
    first: Option<f64>,
    body_ms: f64,
    total_ms: f64,
    version: String,
    fixture_connection: Option<String>,
}
pub struct Bench {
    client: reqwest::Client,
    endpoint: Endpoint,
    credentials: Credentials,
    limits: Limits,
}
impl Bench {
    pub fn new(url: &str, token: &str, limits: Limits) -> Result<Self> {
        if url.len() > 2048 || token.len() > 16384 {
            return Err(Error::Limit("configuration byte limit"));
        }
        if limits.max_bytes == 0
            || limits.max_bytes > 64 * 1024 * 1024
            || limits.max_rows == 0
            || limits.max_rows > 100_000
            || limits.max_value_bytes == 0
            || limits.max_value_bytes > 8192
        {
            return Err(Error::Limit("invalid limits"));
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .http1_only()
                .timeout(Duration::from_secs(10))
                .pool_max_idle_per_host(1)
                .build()
                .map_err(|_| Error::Configuration("client configuration"))?,
            endpoint: Endpoint::parse(url).map_err(|_| Error::Configuration("invalid endpoint"))?,
            credentials: Credentials::bearer(token)
                .map_err(|_| Error::Configuration("invalid bearer credential"))?,
            limits,
        })
    }
    fn request(&self, query: &Query, format: Option<Format>) -> Result<reqwest::Request> {
        let mut req = match format {
            Some(f) => query.build_request(&self.client, &self.endpoint, &self.credentials, f),
            None => query.build_command_request(&self.client, &self.endpoint, &self.credentials),
        }
        .map_err(|_| Error::Configuration("request construction"))?;
        req.headers_mut().insert(
            "accept-encoding",
            reqwest::header::HeaderValue::from_static("identity"),
        );
        Ok(req)
    }
    async fn download(&self, req: reqwest::Request, id: &str) -> Result<Download> {
        let start = Instant::now();
        let mut response = self
            .client
            .execute(req)
            .await
            .map_err(|_| Error::Transport("HTTP request failed"))?;
        let headers_ms = ms(start);
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Transport("HTTP status rejected"));
        }
        if response
            .headers()
            .get("x-clickhouse-query-id")
            .and_then(|v| v.to_str().ok())
            != Some(id)
        {
            return Err(Error::InvalidData("response query ID mismatch"));
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.limits.max_bytes as u64)
        {
            return Err(Error::Limit("body byte limit"));
        }
        if response
            .headers()
            .get("content-encoding")
            .is_some_and(|v| v != "identity")
        {
            return Err(Error::Transport("compressed HTTP response rejected"));
        }
        let version = format!("{:?}", response.version());
        let fixture_connection = response
            .headers()
            .get("x-fixture-connection")
            .and_then(|v| v.to_str().ok())
            .filter(|s| s.len() <= 64)
            .map(str::to_owned);
        let body_start = Instant::now();
        let mut first = None;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Transport("HTTP body failed"))?
        {
            if first.is_none() && !chunk.is_empty() {
                first = Some(ms(start))
            }
            if chunk.len() > self.limits.max_bytes.saturating_sub(bytes.len()) {
                return Err(Error::Limit("body byte limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Download {
            bytes,
            headers_ms,
            first,
            body_ms: ms(body_start),
            total_ms: ms(start),
            version,
            fixture_connection,
        })
    }
    async fn control(
        &self,
        query: Query,
        format: Option<Format>,
        kind: &'static str,
        controls: &mut Vec<Control>,
    ) -> Result<Vec<u8>> {
        let start = Instant::now();
        let id = query.query_id().to_owned();
        let result = self.download(self.request(&query, format)?, &id).await;
        controls.push(Control {
            kind,
            query_id: id,
            wall_ms: ms(start),
            succeeded: result.is_ok(),
        });
        result.map(|d| d.bytes)
    }
    async fn terminal(
        &self,
        id: &str,
        marker: &str,
        controls: &mut Vec<Control>,
    ) -> Result<Engine> {
        let flush = Query::new("SYSTEM FLUSH LOGS", fresh())
            .map_err(|_| Error::Configuration("control construction"))?;
        let _ = self.control(flush, None, "flush_query_log", controls).await;
        for attempt in 0..20 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let sql = "SELECT countIf(type = 'QueryStart') AS starts, countIf(type = 'QueryFinish') AS finishes, countIf(type IN ('ExceptionBeforeStart', 'ExceptionWhileProcessing')) AS errors, max(query_duration_ms) AS query_duration_ms, max(read_rows) AS read_rows, max(read_bytes) AS read_bytes, max(result_rows) AS result_rows, max(if(mapContains(ProfileEvents, 'SelectedMarks'), toNullable(ProfileEvents['SelectedMarks']), NULL)) AS selected_marks, max(if(mapContains(ProfileEvents, 'SelectedParts'), toNullable(ProfileEvents['SelectedParts']), NULL)) AS selected_parts FROM system.query_log WHERE query_id={id:String} AND log_comment={marker:String} AND is_initial_query=1";
            let query = Query::new(sql, fresh())
                .map_err(|_| Error::Configuration("control construction"))?
                .parameter("id", id)
                .map_err(|_| Error::Configuration("control construction"))?
                .parameter("marker", marker)
                .map_err(|_| Error::Configuration("control construction"))?;
            let bytes = self
                .control(query, Some(Format::Json), "terminal_query_log", controls)
                .await?;
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|_| Error::Completion("invalid completion JSON"))?;
            let rows = value["data"]
                .as_array()
                .filter(|a| a.len() == 1)
                .ok_or(Error::Completion("invalid completion rows"))?;
            let row = &rows[0];
            let starts =
                number(&row["starts"]).ok_or(Error::Completion("invalid terminal counts"))?;
            let finishes =
                number(&row["finishes"]).ok_or(Error::Completion("invalid terminal counts"))?;
            let errors =
                number(&row["errors"]).ok_or(Error::Completion("invalid terminal counts"))?;
            if starts <= 1 && finishes == 0 && errors == 0 {
                continue;
            }
            if starts != 1 || finishes != 1 || errors != 0 {
                return Err(Error::Completion("query completion failed or ambiguous"));
            }
            return Ok(Engine {
                query_duration_ms: number(&row["query_duration_ms"]),
                read_rows: number(&row["read_rows"]),
                read_bytes: number(&row["read_bytes"]),
                result_rows: number(&row["result_rows"]),
                selected_marks: number(&row["selected_marks"]),
                selected_parts: number(&row["selected_parts"]),
                ..Engine::default()
            });
        }
        Err(Error::Completion("query completion unconfirmed"))
    }
    pub async fn identity(&self) -> Result<Value> {
        let query = Query::new(
            "SELECT version() AS version, currentUser() AS principal",
            fresh(),
        )
        .map_err(|_| Error::Configuration("identity construction"))?;
        let mut controls = vec![];
        let bytes = self
            .control(query, Some(Format::Json), "identity", &mut controls)
            .await?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| Error::InvalidData("identity JSON"))?;
        let rows = value["data"]
            .as_array()
            .filter(|a| a.len() == 1)
            .ok_or(Error::InvalidData("identity rows"))?;
        // Identity values are trusted fixture metadata but still bounded before output.
        let version = rows[0]["version"]
            .as_str()
            .filter(|s| s.len() < 128)
            .ok_or(Error::InvalidData("identity version"))?;
        let principal = rows[0]["principal"]
            .as_str()
            .filter(|s| s.len() < 128)
            .ok_or(Error::InvalidData("identity principal"))?;
        Ok(serde_json::json!({"version":version,"principal":principal,"controls":controls}))
    }
    pub async fn measure(&self, scenario: &Scenario, format: Format) -> Result<(Decoded, Sample)> {
        if format == Format::Arrow || scenario.rows > self.limits.max_rows {
            return Err(Error::InvalidData("unsupported benchmark request"));
        }
        let start = Instant::now();
        let id = fresh();
        let marker = fresh();
        let query = scenario.query(&id)?;
        let mut req = self.request(&query, Some(format))?;
        // Explicit benchmark-owned logging settings; same for both formats. The
        // public API intentionally reserves these lifecycle settings.
        req.url_mut().query_pairs_mut().extend_pairs([
            ("log_comment", marker.as_str()),
            ("log_queries", "1"),
            ("log_query_settings", "1"),
            ("log_queries_probability", "1"),
            ("log_queries_min_query_duration_ms", "0"),
            ("output_format_arrow_compression_method", "none"),
            ("max_execution_time", "5"),
            ("max_result_rows", "100000"),
            ("max_result_bytes", "16777216"),
            ("result_overflow_mode", "throw"),
            ("max_threads", "1"),
            ("use_query_cache", "0"),
            ("output_format_arrow_string_as_string", "1"),
        ]);
        let d = self.download(req, &id).await?;
        let cpu = cpu_time();
        let decode_start = Instant::now();
        let decoded = decode(&d.bytes, format, scenario, self.limits)?;
        let decode_wall_ms = ms(decode_start);
        let decode_thread_cpu_ms = cpu_delta(cpu);
        let cpu = cpu_time();
        let consumer_start = Instant::now();
        let (rows, result_digest) = consume(&decoded, scenario.count, self.limits)?;
        let consumer_wall_ms = ms(consumer_start);
        let consumer_thread_cpu_ms = cpu_delta(cpu);
        let completion_start = Instant::now();
        let mut controls = vec![];
        let engine = tokio::time::timeout(
            Duration::from_secs(10),
            self.terminal(&id, &marker, &mut controls),
        )
        .await
        .map_err(|_| Error::Completion("completion deadline exceeded"))??;
        let completion_wall_ms = ms(completion_start);
        if engine.result_rows != Some(rows as u64) {
            return Err(Error::Completion("terminal result row mismatch"));
        }
        let batches = match &decoded {
            Decoded::Arrow(b) => Some(b.len()),
            _ => None,
        };
        let sample = Sample {
            query_id: id,
            execution_marker: marker,
            format: format.as_str(),
            headers_ms: d.headers_ms,
            first_body_byte_ms: d.first,
            body_after_headers_ms: d.body_ms,
            request_body_ms: d.total_ms,
            decode_wall_ms,
            decode_thread_cpu_ms,
            consumer_wall_ms,
            consumer_thread_cpu_ms,
            completion_wall_ms,
            total_wall_ms: ms(start),
            bytes: d.bytes.len(),
            rows,
            batches,
            result_digest,
            http_version: d.version,
            fixture_connection: d.fixture_connection,
            controls,
            engine,
            completion: "one QueryStart + one QueryFinish + zero exceptions; ID and execution marker matched",
        };
        Ok((decoded, sample))
    }
    pub fn validate(&self, scenario: &Scenario, decoded: &Decoded) -> Result<()> {
        let rows = extract(decoded, scenario.count)?;
        if rows.len() != scenario.rows {
            return Err(Error::InvalidData("fixture row count mismatch"));
        }
        for (index, row) in rows.iter().enumerate() {
            let expected = if scenario.count {
                Row {
                    id: scenario.source_rows,
                    value: None,
                }
            } else {
                Row {
                    id: index as u64,
                    value: expected_value(index),
                }
            };
            if *row != expected {
                return Err(Error::InvalidData("full fixture value mismatch"));
            }
        }
        Ok(())
    }
}
fn expected_value(index: usize) -> Option<String> {
    match index % 5 {
        0 => None,
        1 => Some(String::new()),
        2 => Some("héllo\n\0".into()),
        3 => Some("slash\\quote\"".into()),
        _ => Some(format!("row-{index}")),
    }
}
fn decode(bytes: &[u8], format: Format, scenario: &Scenario, limits: Limits) -> Result<Decoded> {
    if format == Format::Json {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| Error::InvalidData("invalid JSON result"))?;
        let meta = value["meta"]
            .as_array()
            .ok_or(Error::InvalidData("JSON metadata missing"))?;
        let columns = if scenario.count { 1 } else { 2 };
        if meta.len() != columns
            || meta[0]["name"] != "id"
            || meta[0]["type"] != "UInt64"
            || (!scenario.count
                && (meta[1]["name"] != "value" || meta[1]["type"] != "Nullable(String)"))
        {
            return Err(Error::InvalidData("unsupported JSON schema"));
        }
        let data = value["data"]
            .as_array()
            .ok_or(Error::InvalidData("JSON rows missing"))?;
        if data.len() > limits.max_rows || number(&value["rows"]) != Some(data.len() as u64) {
            return Err(Error::Limit("JSON row limit or count"));
        }
        let mut rows = Vec::with_capacity(data.len());
        for row in data {
            if row.as_object().map(|o| o.len()) != Some(columns)
                || (!scenario.count && row.get("value").is_none())
            {
                return Err(Error::InvalidData("JSON column count"));
            }
            let id = number(&row["id"]).ok_or(Error::InvalidData("invalid UInt64"))?;
            let text = if scenario.count || row["value"].is_null() {
                None
            } else {
                Some(
                    row["value"]
                        .as_str()
                        .filter(|s| s.len() <= limits.max_value_bytes)
                        .ok_or(Error::InvalidData("invalid bounded text"))?
                        .to_owned(),
                )
            };
            rows.push((id, text));
        }
        Ok(Decoded::Json(rows))
    } else {
        check_ipc(bytes, limits.max_rows)?;
        let reader = StreamReader::try_new(Cursor::new(bytes), None)
            .map_err(|_| Error::InvalidData("invalid IPC schema"))?;
        let schema = reader.schema();
        let fields = schema.fields();
        if fields.len() != if scenario.count { 1 } else { 2 }
            || fields[0].name() != "id"
            || fields[0].data_type() != &DataType::UInt64
            || (!scenario.count
                && (fields[1].name() != "value" || fields[1].data_type() != &DataType::Utf8))
        {
            return Err(Error::InvalidData("unsupported Arrow schema"));
        }
        let mut batches = vec![];
        let mut rows = 0usize;
        for batch in reader {
            let batch = batch.map_err(|_| Error::InvalidData("invalid IPC batch"))?;
            rows = rows
                .checked_add(batch.num_rows())
                .ok_or(Error::Limit("row overflow"))?;
            if rows > limits.max_rows || batches.len() >= 1024 {
                return Err(Error::Limit("Arrow row or batch limit"));
            };
            batches.push(batch);
        }
        Ok(Decoded::Arrow(batches))
    }
}
/// Validate all IPC framing and allocation-driving lengths before Arrow reader.
/// Only uncompressed Schema + RecordBatch messages are in this fixture contract.
pub fn check_ipc(bytes: &[u8], max_rows: usize) -> Result<()> {
    let mut pos = 0usize;
    let mut schema = false;
    let mut rows = 0usize;
    let mut batches = 0;
    loop {
        let prefix = bytes
            .get(pos..pos.saturating_add(8))
            .ok_or(Error::InvalidData("missing IPC end marker"))?;
        if prefix[..4] != [255, 255, 255, 255] {
            return Err(Error::Configuration("IPC continuation marker required"));
        }
        let len = u32::from_le_bytes(
            prefix[4..8]
                .try_into()
                .map_err(|_| Error::InvalidData("IPC length"))?,
        ) as usize;
        pos += 8;
        if len == 0 {
            return if schema && pos == bytes.len() {
                Ok(())
            } else {
                Err(Error::Completion("IPC terminal or trailing bytes"))
            };
        }
        if len > 1024 * 1024 {
            return Err(Error::Limit("IPC metadata byte limit"));
        }
        let metadata = bytes
            .get(
                pos..pos
                    .checked_add(len)
                    .ok_or(Error::Limit("IPC length overflow"))?,
            )
            .ok_or(Error::InvalidData("truncated IPC metadata"))?;
        let message = arrow_ipc::root_as_message(metadata)
            .map_err(|_| Error::InvalidData("invalid IPC metadata"))?;
        pos += len;
        let body = usize::try_from(message.bodyLength())
            .map_err(|_| Error::InvalidData("negative IPC body"))?;
        if body > bytes.len().saturating_sub(pos) {
            return Err(Error::InvalidData("truncated IPC body"));
        }
        match message.header_type() {
            MessageHeader::Schema if !schema && batches == 0 && body == 0 => {
                // Reject nested/dictionary schemas before converting to Arrow.
                let wire = message
                    .header_as_schema()
                    .ok_or(Error::InvalidData("IPC schema"))?;
                let fields = wire.fields().ok_or(Error::InvalidData("IPC fields"))?;
                if fields.is_empty() || fields.len() > 2 {
                    return Err(Error::Limit("IPC field limit"));
                }
                for field in fields {
                    if field.dictionary().is_some()
                        || field.children().is_some_and(|c| !c.is_empty())
                        || !matches!(
                            field.type_type(),
                            arrow_ipc::Type::Int | arrow_ipc::Type::Utf8
                        )
                    {
                        return Err(Error::InvalidData("unsupported IPC field"));
                    }
                    if field.type_type() == arrow_ipc::Type::Int
                        && field
                            .type_as_int()
                            .is_none_or(|int| int.bitWidth() != 64 || int.is_signed())
                    {
                        return Err(Error::Configuration("IPC integer must be unsigned 64-bit"));
                    }
                }
                schema = true;
            }
            MessageHeader::RecordBatch if schema => {
                let batch = message
                    .header_as_record_batch()
                    .ok_or(Error::InvalidData("IPC batch"))?;
                if batch.compression().is_some() {
                    return Err(Error::InvalidData("compressed IPC rejected"));
                }
                if batch
                    .variadicBufferCounts()
                    .is_some_and(|counts| !counts.is_empty())
                {
                    return Err(Error::InvalidData(
                        "primitive IPC variadic buffers rejected",
                    ));
                }
                let n = usize::try_from(batch.length())
                    .map_err(|_| Error::InvalidData("negative IPC row count"))?;
                rows = rows
                    .checked_add(n)
                    .ok_or(Error::Limit("IPC row overflow"))?;
                batches += 1;
                if rows > max_rows || batches > 1024 {
                    return Err(Error::Limit("IPC row or batch limit"));
                }
                if batch.nodes().is_none_or(|n| {
                    n.len() > 2
                        || n.iter().any(|node| {
                            node.length() != batch.length()
                                || node.null_count() < 0
                                || node.null_count() > node.length()
                        })
                }) {
                    return Err(Error::InvalidData("unsupported IPC nodes"));
                }
                if batch.buffers().is_none_or(|b| {
                    b.len() > 5
                        || b.iter().any(|buf| {
                            buf.offset() < 0
                                || buf.length() < 0
                                || buf
                                    .offset()
                                    .checked_add(buf.length())
                                    .is_none_or(|end| end > body as i64)
                        })
                }) {
                    return Err(Error::InvalidData("IPC buffer bounds"));
                }
            }
            _ => return Err(Error::InvalidData("unsupported IPC message")),
        }
        pos = pos
            .checked_add(body)
            .ok_or(Error::Limit("IPC length overflow"))?;
    }
}
fn extract(decoded: &Decoded, count: bool) -> Result<Vec<Row>> {
    match decoded {
        Decoded::Json(rows) => Ok(rows
            .iter()
            .map(|(id, value)| Row {
                id: *id,
                value: value.clone(),
            })
            .collect()),
        Decoded::Arrow(batches) => {
            let mut rows = vec![];
            for batch in batches {
                let ids = batch
                    .columns()
                    .first()
                    .ok_or(Error::InvalidData("missing id column"))?
                    .as_any()
                    .downcast_ref::<UInt64Array>()
                    .ok_or(Error::InvalidData("Arrow id type"))?;
                let values = if count {
                    None
                } else {
                    Some(
                        batch
                            .columns()
                            .get(1)
                            .ok_or(Error::InvalidData("missing value column"))?
                            .as_any()
                            .downcast_ref::<StringArray>()
                            .ok_or(Error::InvalidData("Arrow text type"))?,
                    )
                };
                for i in 0..batch.num_rows() {
                    if ids.is_null(i) {
                        return Err(Error::InvalidData("null ID"));
                    };
                    rows.push(Row {
                        id: ids.value(i),
                        value: values.and_then(|v| {
                            if v.is_null(i) {
                                None
                            } else {
                                Some(v.value(i).to_owned())
                            }
                        }),
                    });
                }
            }
            Ok(rows)
        }
    }
}
pub fn consume(decoded: &Decoded, count: bool, limits: Limits) -> Result<(usize, String)> {
    let mut hasher = Sha256::new();
    let mut rows = 0;
    let mut row = |id: u64, text: Option<&str>| -> Result<()> {
        hasher.update(id.to_le_bytes());
        if !count {
            match text {
                None => hasher.update([0]),
                Some(s) => {
                    if s.len() > limits.max_value_bytes {
                        return Err(Error::Limit("value byte limit"));
                    };
                    hasher.update([1]);
                    hasher.update((s.len() as u64).to_le_bytes());
                    hasher.update(s.as_bytes());
                }
            }
        }
        rows += 1;
        Ok(())
    };
    match decoded {
        Decoded::Json(values) => {
            for (id, value) in values {
                row(*id, value.as_deref())?
            }
        }
        // Direct native array iteration: no JSON/intermediate row conversion.
        Decoded::Arrow(batches) => {
            for batch in batches {
                let ids = batch
                    .columns()
                    .first()
                    .ok_or(Error::InvalidData("missing id column"))?
                    .as_any()
                    .downcast_ref::<UInt64Array>()
                    .ok_or(Error::InvalidData("Arrow id type"))?;
                let texts = if count {
                    None
                } else {
                    Some(
                        batch
                            .columns()
                            .get(1)
                            .ok_or(Error::InvalidData("missing value column"))?
                            .as_any()
                            .downcast_ref::<StringArray>()
                            .ok_or(Error::InvalidData("Arrow value type"))?,
                    )
                };
                for i in 0..batch.num_rows() {
                    if ids.is_null(i) {
                        return Err(Error::InvalidData("null ID"));
                    };
                    row(
                        ids.value(i),
                        texts.and_then(|v| if v.is_null(i) { None } else { Some(v.value(i)) }),
                    )?;
                }
            }
        }
    }
    Ok((
        rows,
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}
fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}
fn fresh() -> String {
    Uuid::new_v4().to_string()
}
fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
// CPU instrumentation is unavailable without an additional safe platform API.
// Keep it explicitly null; wall clocks and external RSS observations remain.
fn cpu_time() -> Option<f64> {
    None
}
fn cpu_delta(start: Option<f64>) -> Option<f64> {
    Some(cpu_time()? - start?)
}
#[cfg(test)]
mod tests {
    #[test]
    fn sha256_fingerprints_keep_lowercase_padded_hex() {
        assert_eq!(
            super::hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let (rows, digest) = super::consume(
            &super::Decoded::Json(Vec::new()),
            false,
            super::Limits::default(),
        )
        .unwrap();
        assert_eq!(rows, 0);
        assert_eq!(
            digest,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    fn frame(builder: &flatbuffers::FlatBufferBuilder<'_>) -> Vec<u8> {
        let metadata = builder.finished_data();
        let padded = (metadata.len() + 7) & !7;
        let mut bytes = vec![255; 4];
        bytes.extend_from_slice(&(padded as u32).to_le_bytes());
        bytes.extend_from_slice(metadata);
        bytes.resize(8 + padded, 0);
        bytes
    }
    fn malformed_schema(bit_width: i32) -> Vec<u8> {
        let mut b = flatbuffers::FlatBufferBuilder::new();
        let int = arrow_ipc::Int::create(
            &mut b,
            &arrow_ipc::IntArgs {
                bitWidth: bit_width,
                is_signed: false,
            },
        );
        let name = b.create_string("id");
        let field = arrow_ipc::Field::create(
            &mut b,
            &arrow_ipc::FieldArgs {
                name: Some(name),
                type_type: arrow_ipc::Type::Int,
                type_: Some(int.as_union_value()),
                ..Default::default()
            },
        );
        let fields = b.create_vector(&[field]);
        let schema = arrow_ipc::Schema::create(
            &mut b,
            &arrow_ipc::SchemaArgs {
                fields: Some(fields),
                ..Default::default()
            },
        );
        let message = arrow_ipc::Message::create(
            &mut b,
            &arrow_ipc::MessageArgs {
                version: arrow_ipc::MetadataVersion::V5,
                header_type: arrow_ipc::MessageHeader::Schema,
                header: Some(schema.as_union_value()),
                ..Default::default()
            },
        );
        b.finish(message, None);
        frame(&b)
    }
    #[test]
    fn invalid_integer_width_rejected_before_arrow_converter() {
        let mut bytes = malformed_schema(7);
        bytes.extend_from_slice(&[255, 255, 255, 255, 0, 0, 0, 0]);
        assert!(super::check_ipc(&bytes, 10).is_err());
    }
    #[test]
    fn primitive_variadic_buffers_rejected_before_arrow_reader() {
        let mut bytes = malformed_schema(64);
        let mut b = flatbuffers::FlatBufferBuilder::new();
        let nodes = b.create_vector(&[arrow_ipc::FieldNode::new(0, 0)]);
        let buffers =
            b.create_vector(&[arrow_ipc::Buffer::new(0, 0), arrow_ipc::Buffer::new(0, 0)]);
        let variadic = b.create_vector(&[1i64]);
        let batch = arrow_ipc::RecordBatch::create(
            &mut b,
            &arrow_ipc::RecordBatchArgs {
                nodes: Some(nodes),
                buffers: Some(buffers),
                variadicBufferCounts: Some(variadic),
                ..Default::default()
            },
        );
        let message = arrow_ipc::Message::create(
            &mut b,
            &arrow_ipc::MessageArgs {
                version: arrow_ipc::MetadataVersion::V5,
                header_type: arrow_ipc::MessageHeader::RecordBatch,
                header: Some(batch.as_union_value()),
                ..Default::default()
            },
        );
        b.finish(message, None);
        bytes.extend(frame(&b));
        bytes.extend_from_slice(&[255, 255, 255, 255, 0, 0, 0, 0]);
        assert!(super::check_ipc(&bytes, 10).is_err());
    }
    #[test]
    fn missing_nullable_json_column_is_not_null() {
        let scenario = super::Scenario::page(1).unwrap();
        let bytes = br#"{"meta":[{"name":"id","type":"UInt64"},{"name":"value","type":"Nullable(String)"}],"data":[{"id":"0","other":null}],"rows":1}"#;
        assert!(
            super::decode(
                bytes,
                antalya_rs::Format::Json,
                &scenario,
                super::Limits::default()
            )
            .is_err()
        );
    }
    #[test]
    fn truncated_ipc_is_rejected_before_decode() {
        assert!(super::check_ipc(&[255, 255, 255, 255, 255, 255, 255, 127], 10).is_err());
    }
    #[test]
    fn missing_ipc_terminal_marker_is_rejected() {
        assert!(super::check_ipc(&[], 10).is_err());
    }
}
