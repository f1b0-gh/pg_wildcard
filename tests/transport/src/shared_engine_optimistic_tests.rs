// SPDX-License-Identifier: Apache-2.0
//! The real client and worker consume a controlled HTTP/Arrow server.
use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const PAGE_DIGEST: &str = "42cf670078d289375c7e4dcc888c0c8758359bb3e7e3cc4f3f0f78795e0b32a5";
const PROOF_DELAY: Duration = Duration::from_millis(25);

struct Fixture {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(batches: &[RecordBatch], proof_rows: usize, failed: bool) -> Self {
        let mut ipc = Vec::new();
        let mut writer =
            arrow_ipc::writer::StreamWriter::try_new(&mut ipc, &batches[0].schema()).unwrap();
        for batch in batches {
            writer.write(batch).unwrap();
        }
        writer.finish().unwrap();
        drop(writer);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            while let Ok((mut socket, _)) = listener.accept() {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(&mut socket);
                let mut headers = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse::<usize>().unwrap();
                        assert!(length < 128 * 1024);
                    }
                    headers.push_str(&line);
                }
                let mut request = vec![0; length];
                reader.read_exact(&mut request).unwrap();
                let request = String::from_utf8(request).unwrap();
                let response = if request.contains("system.query_log") {
                    std::thread::sleep(PROOF_DELAY);
                    serde_json::to_vec(&json!({"data":[{"starts":1,"finishes":usize::from(!failed),"errors":usize::from(failed),"cancelled":0,"result_rows":proof_rows,"read_rows":proof_rows,"read_bytes":16}]})).unwrap()
                } else if request.contains("SYSTEM FLUSH LOGS") {
                    Vec::new()
                } else {
                    assert!(request.contains("SELECT"));
                    ipc.clone()
                };
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .unwrap();
                socket.write_all(&response).unwrap();
            }
        });
        Self {
            address,
            stop,
            thread: Some(thread),
        }
    }
    fn client(&self) -> AntalyaClient {
        client(&format!("http://{}", self.address), "test-token").unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn page() -> Vec<RecordBatch> {
    vec![
        super::tests::batch(vec![Some(1)], Some(vec![Some("a")])),
        super::tests::batch(vec![Some(2)], Some(vec![Some("b")])),
    ]
}
fn command() -> Value {
    json!({"engine":"antalya_shared_optimistic","mode":"page","sql":"SELECT id, value","settings":{},"expected_total":2,"expected_rows":2,"expected_digest":PAGE_DIGEST})
}
fn number(sample: &Value, key: &str) -> f64 {
    sample[key]
        .as_f64()
        .unwrap_or_else(|| panic!("missing timing {key}: {sample}"))
}

#[tokio::test]
async fn optimistic_clocks_separate_data_from_private_proof_and_final_verification() {
    let server = Fixture::new(&page(), 2, false);
    let sample = measure(&server.client(), &command()).await.unwrap();
    assert_eq!(sample["result_digest"], PAGE_DIGEST);
    assert_eq!(sample["result_verified"], true);
    assert_eq!(sample["completion"], "Succeeded");
    let keys = [
        "optimistic_first_batch_ms",
        "optimistic_all_rows_ms",
        "optimistic_stream_eof_ms",
        "confirmation_started_ms",
        "confirmed_ms",
        "client_ms",
    ];
    for pair in keys.windows(2) {
        assert!(
            number(&sample, pair[0]) <= number(&sample, pair[1]),
            "{pair:?}: {sample}"
        );
    }
    assert!(number(&sample, "confirmation_ms") >= PROOF_DELAY.as_secs_f64() * 1000.0);
    assert_eq!(
        sample["optimistic_result_status"],
        "ProvisionalUntilConfirmation"
    );
}

#[tokio::test]
async fn complete_preview_with_failed_or_mismatched_terminal_proof_never_returns_sample() {
    for (proof_rows, failed) in [(2, true), (3, false)] {
        let server = Fixture::new(&page(), proof_rows, failed);
        assert!(measure(&server.client(), &command()).await.is_err());
    }
}

#[tokio::test]
async fn confirmed_but_wrong_values_or_row_count_never_return_sample() {
    let server = Fixture::new(&page(), 2, false);
    for (key, wrong) in [
        ("expected_digest", json!("wrong")),
        ("expected_rows", json!(1)),
    ] {
        let mut command = command();
        command[key] = wrong;
        assert!(measure(&server.client(), &command).await.is_err());
    }
}

#[tokio::test]
async fn empty_page_has_no_all_rows_preview_but_zero_count_has_one_result_row() {
    for (mode, batch, expected_rows, digest) in [
        (
            "page",
            super::tests::batch(vec![], Some(vec![])),
            0,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            "count",
            super::tests::batch(vec![Some(0)], None),
            1,
            "af5570f5a1810b7af78caf4bc70a660f0df51e42baf91d4de5b2328de0e83dfc",
        ),
    ] {
        let server = Fixture::new(&[batch], expected_rows, false);
        let mut command = command();
        command["mode"] = json!(mode);
        command["expected_total"] = json!(0);
        command["expected_rows"] = json!(expected_rows);
        command["expected_digest"] = json!(digest);
        let sample = measure(&server.client(), &command).await.unwrap();
        assert_eq!(
            sample["optimistic_all_rows_ms"].is_null(),
            expected_rows == 0
        );
        assert!(number(&sample, "optimistic_stream_eof_ms") <= number(&sample, "confirmed_ms"));
    }
}

#[tokio::test]
async fn strict_path_keeps_terminal_inclusive_result_and_no_optimistic_fields() {
    let server = Fixture::new(&page(), 2, false);
    let mut command = command();
    command["engine"] = json!("antalya_shared");
    let sample = measure(&server.client(), &command).await.unwrap();
    assert_eq!(sample["result_digest"], PAGE_DIGEST);
    assert_eq!(
        sample["client_path"],
        "AntalyaClient::fetch_arrow/next_batch"
    );
    assert!(sample.get("optimistic_stream_eof_ms").is_none());
    assert!(number(&sample, "client_ms") >= PROOF_DELAY.as_secs_f64() * 1000.0);
}
