// SPDX-License-Identifier: Apache-2.0
use antalya_rs::Format;
use arrow_array::{RecordBatch, StringArray, UInt64Array};
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
use wildcard_transport_format::{Bench, Limits, Scenario};

#[derive(Clone, Copy)]
enum Fault {
    None,
    Http,
    Terminal,
    Correlation,
    Truncated,
    Huge,
    Value,
    LateBody,
    ErrorEvent,
    MissingRows,
}
struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    connections: Arc<AtomicUsize>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start(fault: Fault) -> Self {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", socket.local_addr().unwrap());
        socket.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(AtomicUsize::new(0));
        let (end, count) = (stop.clone(), connections.clone());
        let worker = thread::spawn(move || {
            while !end.load(Ordering::Relaxed) {
                let Ok((stream, _)) = socket.accept() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                count.fetch_add(1, Ordering::SeqCst);
                stream
                    .set_read_timeout(Some(Duration::from_millis(100)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                while !end.load(Ordering::Relaxed) {
                    let mut request = String::new();
                    match reader.read_line(&mut request) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(_) => continue,
                    }
                    let mut len = 0;
                    let mut bearer = false;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() {
                            return;
                        }
                        if line == "\r\n" {
                            break;
                        }
                        if line.to_ascii_lowercase().starts_with("content-length:") {
                            len = line.split(':').nth(1).unwrap().trim().parse().unwrap();
                        }
                        if line
                            .to_ascii_lowercase()
                            .starts_with("authorization: bearer fixture-secret")
                        {
                            bearer = true;
                        }
                    }
                    assert!(bearer, "all paths require bearer");
                    assert!(!request.contains("fixture-secret"));
                    let parsed = reqwest::Url::parse(&format!(
                        "http://localhost{}",
                        request.split_whitespace().nth(1).unwrap()
                    ))
                    .unwrap();
                    let id = parsed
                        .query_pairs()
                        .find(|(k, _)| k == "query_id")
                        .unwrap()
                        .1
                        .into_owned();
                    let mut body = vec![0; len];
                    reader.read_exact(&mut body).unwrap();
                    let sql = String::from_utf8(body).unwrap();
                    let control =
                        sql.contains("system.query_log") || sql.starts_with("SYSTEM FLUSH");
                    let mut result = if sql.contains("system.query_log") {
                        format!(r#"{{"data":[{{"starts":"1","finishes":"{}","errors":"{}","query_duration_ms":"2","read_rows":"3","read_bytes":"24","result_rows":"3","selected_marks":null,"selected_parts":null}}]}}"#, if matches!(fault,Fault::Terminal) {2} else if matches!(fault,Fault::ErrorEvent) {0} else {1}, if matches!(fault,Fault::ErrorEvent) {1} else {0}).into_bytes()
                    } else if control {
                        vec![]
                    } else if sql.contains("FORMAT ArrowStream") {
                        arrow()
                    } else {
                        br#"{"meta":[{"name":"id","type":"UInt64"},{"name":"value","type":"Nullable(String)"}],"data":[{"id":"0","value":null},{"id":"1","value":""},{"id":"2","value":"h\u00e9llo\n\u0000"}],"rows":3}"#.to_vec()
                    };
                    if sql.contains("system.query_log") && matches!(fault, Fault::MissingRows) {
                        let mut value: serde_json::Value = serde_json::from_slice(&result).unwrap();
                        value["data"][0]
                            .as_object_mut()
                            .unwrap()
                            .remove("result_rows");
                        result = serde_json::to_vec(&value).unwrap();
                    }
                    if !control {
                        assert!(parsed.query_pairs().any(|(k, _)| k == "log_comment"));
                        if matches!(fault, Fault::Truncated) {
                            result.truncate(result.len().saturating_sub(8));
                        }
                        if matches!(fault, Fault::Huge) {
                            result = vec![b'x'; 1024 * 1024];
                        }
                        if matches!(fault, Fault::Value) {
                            result=br#"{"meta":[{"name":"id","type":"UInt64"},{"name":"value","type":"Nullable(String)"}],"data":[{"id":"0","value":null},{"id":"1","value":""},{"id":"2","value":"WRONG"}],"rows":3}"#.to_vec();
                        }
                    }
                    let status = if !control && matches!(fault, Fault::Http) {
                        500
                    } else {
                        200
                    };
                    let response_id = if !control && matches!(fault, Fault::Correlation) {
                        "wrong"
                    } else {
                        &id
                    };
                    write!(reader.get_mut(),"HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nX-ClickHouse-Query-Id: {response_id}\r\nX-Fixture-Connection: 1\r\n\r\n", result.len()).unwrap();
                    if !control && matches!(fault, Fault::LateBody) {
                        let end = result.len().saturating_sub(8);
                        let _ = reader.get_mut().write_all(&result[..end]);
                        let _ = reader.get_mut().flush();
                        break;
                    }
                    if reader.get_mut().write_all(&result).is_err() {
                        break;
                    }
                    if reader.get_mut().flush().is_err() {
                        break;
                    }
                }
            }
        });
        Self {
            url,
            stop,
            connections,
            worker: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn arrow() -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::UInt64, false),
        Field::new("value", DataType::Utf8, true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt64Array::from(vec![0, 1, 2])),
            Arc::new(StringArray::from(vec![None, Some(""), Some("héllo\n\0")])),
        ],
    )
    .unwrap();
    let mut bytes = vec![];
    {
        let mut writer = StreamWriter::try_new(&mut bytes, &schema).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
    }
    bytes
}
#[tokio::test]
async fn same_pool_reuses_observed_http1_connection_for_json_arrow_and_controls() {
    let server = Server::start(Fault::None);
    let bench = Bench::new(&server.url, "fixture-secret", Limits::default()).unwrap();
    let scenario = Scenario::page(3).unwrap();
    let (json, j) = bench.measure(&scenario, Format::Json).await.unwrap();
    let (arrow, a) = bench.measure(&scenario, Format::ArrowStream).await.unwrap();
    bench.validate(&scenario, &json).unwrap();
    bench.validate(&scenario, &arrow).unwrap();
    assert_eq!(j.result_digest, a.result_digest);
    assert_ne!(j.query_id, a.query_id);
    assert_eq!(j.controls.len(), 2);
    assert_eq!(a.controls.len(), 2);
    assert_eq!(j.engine.read_rows, Some(3));
    assert_eq!(j.engine.selected_marks, None);
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn provisional_results_never_survive_failures_or_wrong_identity() {
    for fault in [
        Fault::Http,
        Fault::Terminal,
        Fault::Correlation,
        Fault::Truncated,
        Fault::Huge,
        Fault::LateBody,
        Fault::ErrorEvent,
        Fault::MissingRows,
    ] {
        let server = Server::start(fault);
        let bench = Bench::new(
            &server.url,
            "fixture-secret",
            Limits {
                max_bytes: 100_000,
                ..Limits::default()
            },
        )
        .unwrap();
        assert!(
            bench
                .measure(&Scenario::page(3).unwrap(), Format::ArrowStream)
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn full_value_validation_catches_wrong_non_id_value() {
    let server = Server::start(Fault::Value);
    let bench = Bench::new(&server.url, "fixture-secret", Limits::default()).unwrap();
    let scenario = Scenario::page(3).unwrap();
    let (value, _) = bench.measure(&scenario, Format::Json).await.unwrap();
    assert!(bench.validate(&scenario, &value).is_err());
}

#[test]
fn cli_errors_never_echo_bearer_or_credential_url() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wildcard-transport-format"))
        .env_clear()
        .env(
            "ANTALYA_TEST_URL",
            "http://secret-user:secret-password@127.0.0.1/",
        )
        .env("ANTALYA_TEST_TOKEN", "super-secret-token")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for secret in ["super-secret-token", "secret-user", "secret-password"] {
        assert!(!report.contains(secret));
    }
}
