// SPDX-License-Identifier: Apache-2.0
//! Adaptive execution lifecycle tests with bounded, explicitly simulated proof.
use super::*;
use arrow_array::{RecordBatch, StringArray};
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
};

const EXACT: &str = "SELECT toUInt64(1) n FROM fixture.events WHERE n = 1";
const CANDIDATE: &str = "SELECT toUInt64(1) n FROM fixture.events PREWHERE n = 1 WHERE n = 1";
const SUCCESS: &[u8] = br#"{"data":[{"starts":"1","finishes":"1","errors":"0","cancelled":"0","read_rows":"1","read_bytes":"8","result_rows":"1"}]}"#;
const FAILURE: &[u8] = br#"{"data":[{"starts":"1","finishes":"0","errors":"1","cancelled":"0","read_rows":"1","read_bytes":"8","result_rows":"1"}]}"#;

#[derive(Clone, Copy)]
enum Behavior {
    Select,
    Unknown,
    Unauthorized,
    LateFailure,
    HoldProbe,
    Delayed(Duration),
}

#[derive(Default)]
struct Requests {
    probes: AtomicUsize,
    data: AtomicUsize,
    candidates: AtomicUsize,
    observations: AtomicUsize,
}

struct Fixture {
    options: HashMap<String, String>,
    requests: Arc<Requests>,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new(behavior: Behavior) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Requests::default());
        let stop = Arc::new(AtomicBool::new(false));
        let seen = requests.clone();
        let stopping = stop.clone();
        let task = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            let baseline = plan_arrow(128);
            let candidate = plan_arrow(16);
            let unknown = string_arrow(r#"[{"Plan":{"Node Type":"ReadFromMemory"}}]"#);
            while !stopping.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "fixture lifetime exceeded");
                let mut socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => panic!("fixture accept failed"),
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let sql = request_body(&mut socket);
                let (status, content_type, body) = if sql.starts_with("SYSTEM FLUSH LOGS") {
                    (200, "text/plain", &b""[..])
                } else if sql.contains("system.query_log") {
                    seen.observations.fetch_add(1, Ordering::SeqCst);
                    let body = if matches!(behavior, Behavior::LateFailure) {
                        FAILURE
                    } else {
                        SUCCESS
                    };
                    (200, "application/json", body)
                } else {
                    let probe = sql.starts_with("EXPLAIN ");
                    if probe {
                        seen.probes.fetch_add(1, Ordering::SeqCst);
                    } else {
                        assert!(sql.starts_with("SELECT "), "unexpected fixture request");
                        seen.data.fetch_add(1, Ordering::SeqCst);
                        if sql.contains(" PREWHERE ") {
                            seen.candidates.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    if matches!(behavior, Behavior::HoldProbe) && probe {
                        while !stopping.load(Ordering::SeqCst) && Instant::now() < deadline {
                            thread::sleep(Duration::from_millis(2));
                        }
                        break;
                    }
                    if let Behavior::Delayed(delay) = behavior {
                        let until = Instant::now() + delay;
                        while Instant::now() < until && !stopping.load(Ordering::SeqCst) {
                            thread::sleep(Duration::from_millis(2));
                        }
                    }
                    if stopping.load(Ordering::SeqCst) {
                        break;
                    }
                    if probe && matches!(behavior, Behavior::Unauthorized) {
                        (401, "text/plain", &b"refused"[..])
                    } else {
                        let body = if !probe {
                            &include_bytes!("../../tests/fixtures/one-u64.arrow")[..]
                        } else if matches!(behavior, Behavior::Unknown) {
                            unknown.as_slice()
                        } else if sql.contains(" PREWHERE ") {
                            candidate.as_slice()
                        } else {
                            baseline.as_slice()
                        };
                        (200, "application/vnd.apache.arrow.stream", body)
                    }
                };
                // Cancellation/timeout deliberately closes an in-flight response.
                if write!(
                    socket,
                    "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .is_ok()
                {
                    let _ = socket.write_all(body);
                }
            }
        });
        Self {
            options: HashMap::from([
                ("url".into(), format!("http://{address}")),
                ("bearer_token".into(), "adaptive-fixture-a".into()),
                ("timeout_ms".into(), "5000".into()),
            ]),
            requests,
            stop,
            task: Some(task),
        }
    }

    fn wait_for_probe(&self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.requests.probes.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "probe was not dispatched");
            thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(task) = self.task.take() {
            let result = task.join();
            if !thread::panicking() {
                assert!(result.is_ok(), "fixture thread failed");
            }
        }
    }
}

fn request_body(socket: &mut TcpStream) -> String {
    let mut request = Vec::new();
    loop {
        let mut buffer = [0; 4096];
        let count = socket
            .read(&mut buffer)
            .expect("fixture request read failed");
        assert!(count > 0, "fixture request ended early");
        request.extend_from_slice(&buffer[..count]);
        assert!(
            request.len() <= 128 * 1024,
            "fixture request exceeded limit"
        );
        if let Some(end) = request.windows(4).position(|s| s == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&request[..end])
                .unwrap()
                .to_ascii_lowercase();
            assert!(
                headers.contains("authorization: bearer adaptive-fixture-a")
                    || headers.contains("authorization: bearer adaptive-fixture-b"),
                "fixture authentication absent"
            );
            let length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .expect("fixture request has no length");
            assert!(length <= 64 * 1024, "fixture body exceeded limit");
            if request.len() >= end + 4 + length {
                return std::str::from_utf8(&request[end + 4..end + 4 + length])
                    .unwrap()
                    .to_owned();
            }
        }
    }
}

fn string_arrow(text: &str) -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![Field::new(
        "explain",
        DataType::Utf8,
        false,
    )]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(StringArray::from(vec![text]))],
    )
    .unwrap();
    let mut bytes = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut bytes, &schema).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
    }
    bytes
}

fn plan_arrow(selected: u64) -> Vec<u8> {
    string_arrow(&format!(
        r#"[{{"Plan":{{"Node Type":"ReadFromMergeTree","Description":"fixture.events","Indexes":[{{"Type":"PrimaryKey","Initial Granules":128,"Selected Granules":{selected}}}]}}}}]"#
    ))
}

fn drain(stream: &mut Stream) -> Result<u64, antalya_rs::Error> {
    let mut rows = 0;
    let mut schema = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match stream.receive(|| assert!(Instant::now() < deadline, "test receive deadline"))? {
            Poll::Schema(_) => {
                assert!(!schema, "duplicate data schema");
                schema = true;
            }
            Poll::Batch(batch) => {
                assert!(schema, "data before schema");
                rows += batch.num_rows() as u64;
            }
            Poll::Complete => {
                assert!(schema, "completion without data schema");
                return Ok(rows);
            }
            Poll::Pending => unreachable!("receive waits internally"),
        }
    }
}

#[test]
fn confirmed_unknown_plan_falls_back_exact_and_caches_the_preference() {
    let fixture = Fixture::new(Behavior::Unknown);
    let session = Session::from_options(&fixture.options).unwrap();
    for (probes, hits) in [(1, 0), (0, 1)] {
        let mut stream = session
            .start_selected(EXACT, Some(CANDIDATE), None)
            .unwrap();
        assert_eq!(drain(&mut stream).unwrap(), 1);
        let metrics = stream.metrics();
        assert_eq!(metrics.preparation.probe_queries, probes);
        assert_eq!(metrics.preparation.cache_hits, hits);
        assert!(!metrics.preparation.selected);
        assert_eq!(metrics.preparation.reason, "missing_estimate");
        if probes != 0 {
            assert!(metrics.preparation.controls > 0, "probe must be confirmed");
        }
    }
    assert_eq!(fixture.requests.probes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.requests.data.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.requests.candidates.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.requests.observations.load(Ordering::SeqCst), 3);
}

#[test]
fn first_probe_authentication_failure_does_not_dispatch_data() {
    let fixture = Fixture::new(Behavior::Unauthorized);
    let session = Session::from_options(&fixture.options).unwrap();
    let mut stream = session
        .start_selected(EXACT, Some(CANDIDATE), None)
        .unwrap();
    assert!(drain(&mut stream).is_err());
    assert_eq!(fixture.requests.probes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.requests.data.load(Ordering::SeqCst), 0);
}

#[test]
fn late_probe_terminal_failure_does_not_dispatch_data() {
    let fixture = Fixture::new(Behavior::LateFailure);
    let session = Session::from_options(&fixture.options).unwrap();
    let mut stream = session
        .start_selected(EXACT, Some(CANDIDATE), None)
        .unwrap();
    assert_eq!(
        drain(&mut stream).unwrap_err().kind(),
        antalya_rs::ErrorKind::QueryFailed
    );
    assert_eq!(fixture.requests.probes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.requests.data.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.requests.observations.load(Ordering::SeqCst), 1);
}

#[test]
fn cancellation_during_probe_releases_task_and_scan_permit() {
    let fixture = Fixture::new(Behavior::HoldProbe);
    let session = Session::from_options(&fixture.options).unwrap();
    let _other_scans = session.limiter.try_acquire_many(3).unwrap();
    let mut stream = session
        .start_selected(EXACT, Some(CANDIDATE), None)
        .unwrap();
    fixture.wait_for_probe();
    assert_eq!(session.limiter.available_permits(), 0);
    stream.close(|| {}).unwrap();
    assert!(stream.task.is_finished());
    assert_eq!(session.limiter.available_permits(), 1);
    assert_eq!(fixture.requests.data.load(Ordering::SeqCst), 0);
}

#[test]
fn probes_and_data_share_one_deadline() {
    let mut fixture = Fixture::new(Behavior::Delayed(Duration::from_millis(600)));
    fixture.options.insert("timeout_ms".into(), "1500".into());
    let session = Session::from_options(&fixture.options).unwrap();
    let mut stream = session
        .start_selected(EXACT, Some(CANDIDATE), None)
        .unwrap();
    assert_eq!(
        drain(&mut stream).unwrap_err().kind(),
        antalya_rs::ErrorKind::Timeout
    );
    assert_eq!(fixture.requests.probes.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.requests.data.load(Ordering::SeqCst), 1);
    let metrics = stream.metrics();
    assert_eq!(metrics.preparation.probe_queries, 2);
    assert!(metrics.preparation.selected);
    assert!(metrics.execution.is_none(), "data was not confirmed");
}

#[test]
fn synchronous_work_cannot_complete_after_the_shared_deadline() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(10);
        let result = super::complete_before(deadline, async {
            // Model a non-preemptible decoder/parser that does not yield.
            std::thread::sleep(Duration::from_millis(30));
            Ok(())
        })
        .await;
        assert_eq!(result, Err(antalya_rs::Error::Timeout));
    });
}

#[test]
fn plan_cache_isolated_by_credentials_and_effective_settings() {
    let mut fixture = Fixture::new(Behavior::Select);
    let first = Session::from_options(&fixture.options).unwrap();
    let same = Session::from_options(&fixture.options).unwrap();
    assert!(Arc::ptr_eq(&first.plans, &same.plans));
    fixture
        .options
        .insert("bearer_token".into(), "adaptive-fixture-b".into());
    let rotated = Session::from_options(&fixture.options).unwrap();
    assert!(!Arc::ptr_eq(&first.plans, &rotated.plans));
    for (session, settings, probes, hits) in [
        (&first, "max_threads=1", 2, 0),
        (&same, "max_threads=1", 0, 1),
        (&rotated, "max_threads=1", 2, 0),
        (&first, "max_threads=2", 2, 0),
        (&first, "max_threads=1", 0, 1),
    ] {
        let mut stream = session
            .start_selected(EXACT, Some(CANDIDATE), Some(settings))
            .unwrap();
        assert_eq!(drain(&mut stream).unwrap(), 1);
        let metrics = stream.metrics();
        assert_eq!(metrics.preparation.probe_queries, probes);
        assert_eq!(metrics.preparation.cache_hits, hits);
        assert!(metrics.preparation.selected);
    }
    assert_eq!(fixture.requests.probes.load(Ordering::SeqCst), 6);
    assert_eq!(fixture.requests.data.load(Ordering::SeqCst), 5);
    assert_eq!(fixture.requests.candidates.load(Ordering::SeqCst), 5);
}
