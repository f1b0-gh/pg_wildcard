// SPDX-License-Identifier: Apache-2.0
//! Backend-owned runtime/client cache. This module never calls PostgreSQL.
#[path = "adaptive.rs"]
mod adaptive;
#[cfg(test)]
#[path = "adaptive_tests.rs"]
mod adaptive_tests;
#[path = "signals.rs"]
mod signals;
use antalya_rs::{AntalyaClient, BudgetedBatch, Config, Credentials, Endpoint, Query};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll as TaskPoll, Wake, Waker},
    time::{Duration, Instant},
};
use tokio::{
    runtime::Runtime,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
    task::JoinHandle,
};

pub const MAX_BATCH_BYTES: usize = 16 * 1024 * 1024;
const CACHE_CAP: usize = 4;
#[derive(Clone, PartialEq, Eq)]
struct Identity {
    url: String,
    token: String,
    database: String,
    timeout: u64,
    terminal: antalya_rs::TerminalStrategy,
    batch_bytes: usize,
}
pub struct Session {
    runtime: Arc<Runtime>,
    client: AntalyaClient,
    limiter: Arc<Semaphore>,
    plans: adaptive::SharedCache,
    timeout: Duration,
}
struct Cache {
    runtime: Arc<Runtime>,
    limiter: Arc<Semaphore>,
    clients: VecDeque<(Identity, AntalyaClient, adaptive::SharedCache)>,
}
thread_local! { static CACHE: RefCell<Option<Cache>> = const { RefCell::new(None) }; }

impl Session {
    pub fn from_options(o: &HashMap<String, String>) -> Result<Self, &'static str> {
        if o.contains_key("user") || o.contains_key("password") {
            return Err(
                "legacy user/password mappings are unsupported; replace them with bearer_token",
            );
        }
        let id = Identity {
            url: o
                .get("url")
                .cloned()
                .unwrap_or_else(|| "http://127.0.0.1:8123".into()),
            token: o
                .get("bearer_token")
                .filter(|s| !s.is_empty())
                .cloned()
                .ok_or("user mapping option bearer_token is required")?,
            database: o
                .get("database")
                .cloned()
                .unwrap_or_else(|| "default".into()),
            terminal: match o
                .get("terminal_policy")
                .map(String::as_str)
                .unwrap_or("flush_first")
            {
                "flush_first" => antalya_rs::TerminalStrategy::FlushFirst,
                "observe_first" => antalya_rs::TerminalStrategy::ObserveFirst,
                "observe_only" => antalya_rs::TerminalStrategy::ObserveOnly,
                "in_band" => antalya_rs::TerminalStrategy::InBand,
                _ => return Err("invalid terminal_policy"),
            },
            batch_bytes: batch_bytes(o)?,
            timeout: o
                .get("timeout_ms")
                .map(|s| s.parse::<u64>())
                .transpose()
                .map_err(|_| "invalid timeout_ms")?
                .unwrap_or(60_000),
        };
        if !(1..=300_000).contains(&id.timeout) {
            return Err("timeout_ms must be between 1 and 300000");
        }
        // Validate even on cache hits. Neither errors nor Debug echo credentials.
        let config = Config::new(
            Endpoint::parse(&id.url).map_err(|_| "invalid Antalya endpoint")?,
            Credentials::bearer(&id.token).map_err(|_| "invalid bearer_token")?,
        )
        .database(&id.database)
        .map_err(|_| "invalid database")?
        .execution_limits(antalya_rs::ExecutionLimits {
            max_batch_bytes: id.batch_bytes,
            ..Default::default()
        })
        .timeout(Duration::from_millis(id.timeout))
        .terminal_policy(antalya_rs::TerminalPolicy {
            strategy: id.terminal,
            ..Default::default()
        });
        CACHE.with(|cell| {
            let mut cache = cell.borrow_mut();
            if cache.is_none() {
                let runtime = signals::build_runtime()?;
                *cache = Some(Cache {
                    runtime: Arc::new(runtime),
                    limiter: Arc::new(Semaphore::new(4)),
                    clients: VecDeque::new(),
                });
            }
            let cache = cache.as_mut().ok_or("cannot create Antalya runtime")?;
            let (client, plans) = if let Some(pos) =
                cache.clients.iter().position(|(key, _, _)| *key == id)
            {
                let (_, client, plans) = cache.clients.remove(pos).ok_or("invalid client cache")?;
                (client, plans)
            } else {
                let _guard = cache.runtime.enter();
                (
                    AntalyaClient::new(config).map_err(|_| "cannot create Antalya client")?,
                    Arc::new(Mutex::new(adaptive::PlanCache::default())),
                )
            };
            let timeout = Duration::from_millis(id.timeout);
            cache.clients.push_back((id, client.clone(), plans.clone()));
            if cache.clients.len() > CACHE_CAP {
                cache.clients.pop_front();
            }
            Ok(Session {
                runtime: cache.runtime.clone(),
                client,
                limiter: cache.limiter.clone(),
                plans,
                timeout,
            })
        })
    }
    #[cfg(test)]
    pub fn start(&self, sql: &str, settings: Option<&str>) -> Result<Stream, &'static str> {
        self.start_selected(sql, None, settings)
    }
    pub fn start_selected(
        &self,
        sql: &str,
        candidate: Option<&str>,
        settings: Option<&str>,
    ) -> Result<Stream, &'static str> {
        let query = make_query(sql, settings)?;
        let alternative = candidate.map(|s| make_query(s, settings)).transpose()?;
        let exact_sql = sql.to_owned();
        let candidate = candidate.map(str::to_owned);
        let settings = settings.map(str::to_owned);
        let plans = self.plans.clone();
        let deadline = tokio::time::Instant::from_std(Instant::now() + self.timeout);
        let permit = self
            .limiter
            .clone()
            .try_acquire_owned()
            .map_err(|_| "at most four concurrent Antalya scans are supported per backend")?;
        let client = self.client.clone();
        let (producer, rx, terminal) = Producer::channel(Some(permit));
        let metrics = Arc::new(Mutex::new(StreamMetrics::default()));
        let producer_metrics = metrics.clone();
        let task = self.runtime.spawn(async move {
            // One wall-clock budget encloses plan probes, data, backpressure and proof.
            let result = complete_before(deadline, async {
                let selected = if let Some(candidate) = candidate.as_deref() {
                    let preparation = adaptive::select(
                        &client,
                        &plans,
                        &exact_sql,
                        candidate,
                        settings.as_deref(),
                    )
                    .await?;
                    let selected = preparation.selected;
                    producer_metrics
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .preparation = preparation;
                    selected
                } else {
                    false
                };
                check_deadline(deadline)?;
                let query = if selected {
                    alternative.ok_or(antalya_rs::Error::InvalidRequest)?
                } else {
                    query
                };
                let mut execution = client.fetch_arrow(query).await?;
                {
                    let mut m = producer_metrics.lock().unwrap_or_else(|e| e.into_inner());
                    m.last_query_id = execution.query_id().to_owned();
                    m.executions = 1;
                }
                let mut schema_sent = false;
                loop {
                    let batch = execution.next_batch().await?;
                    check_deadline(deadline)?;
                    if !schema_sent {
                        let schema = execution.schema().ok_or(antalya_rs::Error::InvalidIpc)?;
                        send(&producer.tx, Ok(Event::Schema(schema)), &producer_metrics).await?;
                        schema_sent = true;
                    }
                    let Some(batch) = batch else {
                        break;
                    };
                    if batch.charged_bytes() > MAX_BATCH_BYTES {
                        return Err(antalya_rs::Error::LimitExceeded);
                    }
                    {
                        let mut m = producer_metrics.lock().unwrap_or_else(|e| e.into_inner());
                        m.peak_batch_bytes = m.peak_batch_bytes.max(batch.charged_bytes() as u64);
                    }
                    send(&producer.tx, Ok(Event::Batch(batch)), &producer_metrics).await?;
                }
                producer_metrics
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .execution = Some(execution.metrics());
                Ok(())
            })
            .await;
            producer.finish(result);
        });
        Ok(Stream {
            rx,
            terminal,
            task,
            metrics,
        })
    }
}
async fn complete_before(
    deadline: tokio::time::Instant,
    future: impl Future<Output = Result<(), antalya_rs::Error>>,
) -> Result<(), antalya_rs::Error> {
    check_deadline(deadline)?;
    tokio::time::timeout_at(deadline, future)
        .await
        .unwrap_or(Err(antalya_rs::Error::Timeout))?;
    // Tokio timeouts are cooperative: a synchronous decode/parse may return
    // Ready after the deadline without giving the timer a chance to run.
    check_deadline(deadline)
}
fn check_deadline(deadline: tokio::time::Instant) -> Result<(), antalya_rs::Error> {
    if tokio::time::Instant::now() >= deadline {
        Err(antalya_rs::Error::Timeout)
    } else {
        Ok(())
    }
}
fn make_query(sql: &str, settings: Option<&str>) -> Result<Query, &'static str> {
    let mut query = Query::new(sql, format!("wildcard-{}", uuid::Uuid::new_v4()))
        .map_err(|_| "invalid query envelope")?
        // Keep the existing default; only the bounded option below may replace it.
        .setting("max_block_size", "1024")
        .map_err(|_| "invalid FDW setting")?;
    if let Some(settings) = settings {
        if settings.len() > 8192 {
            return Err("settings exceeds limit");
        }
        for pair in settings.split(',') {
            let (key, value) = pair
                .split_once('=')
                .ok_or("settings must be comma-separated key=value pairs")?;
            let key = key.trim();
            let value = value.trim();
            // Deliberately narrow; arbitrary query/security/output settings do not belong in an FDW option.
            if !matches!(
                key,
                "max_threads"
                    | "max_block_size"
                    | "max_execution_time"
                    | "object_storage_cluster"
                    | "iceberg_snapshot_id"
            ) || value.is_empty()
                || value.chars().any(char::is_control)
            {
                return Err("unsupported FDW setting");
            }
            let value = if key == "object_storage_cluster" {
                value.trim_matches('\'')
            } else {
                value
            };
            let valid = match key {
                "max_block_size" => value
                    .parse::<u16>()
                    .ok()
                    .is_some_and(|n| (1..=16384).contains(&n)),
                "max_threads" => value
                    .parse::<u16>()
                    .ok()
                    .is_some_and(|n| (1..=64).contains(&n)),
                "max_execution_time" => value
                    .parse::<f64>()
                    .ok()
                    .is_some_and(|n| n.is_finite() && n > 0.0 && n <= 300.0),
                "iceberg_snapshot_id" => value.parse::<i64>().ok().is_some_and(|n| n > 0),
                "object_storage_cluster" => {
                    !value.is_empty()
                        && value.len() <= 128
                        && value
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                }
                _ => false,
            };
            if !valid {
                return Err("invalid FDW setting value");
            }
            query = query
                .setting(key, value)
                .map_err(|_| "invalid FDW setting")?;
        }
    }
    for (key, value) in [
        ("max_result_rows", "1000000"),
        ("max_result_bytes", "67108864"),
        ("result_overflow_mode", "throw"),
        ("output_format_arrow_string_as_string", "0"),
        ("output_format_arrow_low_cardinality_as_dictionary", "0"),
        ("session_timezone", "UTC"),
        ("cancel_http_readonly_queries_on_client_close", "1"),
    ] {
        query = query
            .setting(key, value)
            .map_err(|_| "invalid FDW setting")?;
    }
    Ok(query)
}
fn batch_bytes(o: &HashMap<String, String>) -> Result<usize, &'static str> {
    let bytes = o
        .get("batch_bytes")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|_| "invalid batch_bytes")?
        .unwrap_or(MAX_BATCH_BYTES);
    if !(65536..=MAX_BATCH_BYTES).contains(&bytes) {
        return Err("batch_bytes must be between 65536 and 16777216");
    }
    Ok(bytes)
}

#[derive(Clone, Default)]
pub struct StreamMetrics {
    pub last_query_id: String,
    pub executions: u64,
    pub peak_batch_bytes: u64,
    pub preparation: adaptive::Preparation,
    pub consumer_wait: Duration,
    pub consumer_waits: u64,
    pub producer_send_wait: Duration,
    pub batches: u64,
    pub rows: u64,
    pub charged_bytes: u64,
    pub execution: Option<antalya_rs::ExecutionMetrics>,
}

impl StreamMetrics {
    /// Accumulate across rescans. Only phase totals of `execution` are retained;
    /// absolute first-batch/EOF offsets cannot be combined across executions.
    pub fn accumulate(&mut self, other: &Self) {
        if other.executions > 0 {
            self.last_query_id.clone_from(&other.last_query_id);
        }
        self.executions = self.executions.saturating_add(other.executions);
        if self.executions != 1
            && let Some(execution) = &mut self.execution
        {
            execution.first_batch = None;
        }
        self.peak_batch_bytes = self.peak_batch_bytes.max(other.peak_batch_bytes);
        self.preparation.accumulate(&other.preparation);
        self.consumer_wait = self.consumer_wait.saturating_add(other.consumer_wait);
        self.producer_send_wait = self
            .producer_send_wait
            .saturating_add(other.producer_send_wait);
        self.consumer_waits = self.consumer_waits.saturating_add(other.consumer_waits);
        self.batches = self.batches.saturating_add(other.batches);
        self.rows = self.rows.saturating_add(other.rows);
        self.charged_bytes = self.charged_bytes.saturating_add(other.charged_bytes);
        if let Some(next) = &other.execution {
            let total = self.execution.get_or_insert_with(Default::default);
            total.first_batch = if self.executions == 1 {
                next.first_batch
            } else {
                None
            };
            total.terminal = total.terminal.saturating_add(next.terminal);
            total.data_poll = total.data_poll.saturating_add(next.data_poll);
            if let Some(t) = next.transfer {
                total.transfer = Some(total.transfer.unwrap_or_default().saturating_add(t));
            }
            if let Some(t) = next.decode {
                total.decode = Some(total.decode.unwrap_or_default().saturating_add(t));
            }
            total.framing = total.framing.saturating_add(next.framing);
            total.wire_bytes = total.wire_bytes.saturating_add(next.wire_bytes);
            total.control_requests = total.control_requests.saturating_add(next.control_requests);
        }
    }
}

async fn send(
    tx: &mpsc::Sender<Result<Event, antalya_rs::Error>>,
    event: Result<Event, antalya_rs::Error>,
    metrics: &Mutex<StreamMetrics>,
) -> Result<(), antalya_rs::Error> {
    let start = Instant::now();
    // Receiver closure is cancellation, never proof of successful execution.
    let sent = tx
        .send(event)
        .await
        .map_err(|_| antalya_rs::Error::Unconfirmed);
    let mut m = metrics.lock().unwrap_or_else(|e| e.into_inner());
    m.producer_send_wait = m.producer_send_wait.saturating_add(start.elapsed());
    sent
}

struct BackendWake(std::thread::Thread);
impl Wake for BackendWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

type EventReceiver = Arc<Mutex<mpsc::Receiver<Result<Event, antalya_rs::Error>>>>;
type TerminalReceiver = oneshot::Receiver<Result<(), antalya_rs::Error>>;

/// The terminal receipt has its own slot and never waits for queue capacity.
/// Shared receiver ownership lets a failed producer revoke unread provisional
/// payloads even when PostgreSQL is not polling. No worker calls PostgreSQL.
struct Producer {
    tx: mpsc::Sender<Result<Event, antalya_rs::Error>>,
    rx: EventReceiver,
    terminal: Option<oneshot::Sender<Result<(), antalya_rs::Error>>>,
    // Keep admission until payload cleanup and terminal publication finish.
    _permit: Option<OwnedSemaphorePermit>,
}
impl Producer {
    fn channel(permit: Option<OwnedSemaphorePermit>) -> (Self, EventReceiver, TerminalReceiver) {
        let (tx, rx) = mpsc::channel(1);
        let rx = Arc::new(Mutex::new(rx));
        let (terminal, receipt) = oneshot::channel();
        (
            Self {
                tx,
                rx: rx.clone(),
                terminal: Some(terminal),
                _permit: permit,
            },
            rx,
            receipt,
        )
    }
    fn finish(mut self, result: Result<(), antalya_rs::Error>) {
        self.publish(result);
    }
    fn publish(&mut self, result: Result<(), antalya_rs::Error>) {
        // Preserve data ordering for ordinary confirmed errors. A timeout or
        // lost producer revokes unread data so its backing cannot outlive cleanup.
        let discard = matches!(
            result,
            Err(antalya_rs::Error::Timeout | antalya_rs::Error::Unconfirmed)
        );
        let mut rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
        if discard {
            rx.close();
            while rx.try_recv().is_ok() {}
        }
        // Publish while holding the receive lock: a closed empty queue must
        // never become visible before its exact terminal status is available.
        if let Some(terminal) = self.terminal.take() {
            let _ = terminal.send(result);
        }
    }
}
impl Drop for Producer {
    fn drop(&mut self) {
        if self.terminal.is_some() {
            // Includes panic, runtime shutdown and abort before the first poll.
            self.publish(Err(antalya_rs::Error::Unconfirmed));
        }
    }
}

pub struct Stream {
    rx: EventReceiver,
    terminal: TerminalReceiver,
    task: JoinHandle<()>,
    metrics: Arc<Mutex<StreamMetrics>>,
}
enum Event {
    Schema(arrow_schema::SchemaRef),
    Batch(BudgetedBatch),
    Complete,
}
pub enum Poll {
    #[cfg(test)]
    Pending,
    Schema(arrow_schema::SchemaRef),
    Batch(BudgetedBatch),
    Complete,
}
impl Stream {
    /// Wait for local task destruction, not for server cancellation. A timeout
    /// leaves its permit owned by the old task until Tokio can destroy it.
    pub fn close(&mut self, mut check_interrupt: impl FnMut()) -> Result<(), &'static str> {
        {
            let mut rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
            rx.close();
            while rx.try_recv().is_ok() {}
        }
        self.task.abort();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.task.is_finished() {
            check_interrupt();
            if Instant::now() >= deadline {
                return Err("Antalya worker cancellation is still pending");
            }
            let waker = Waker::from(Arc::new(BackendWake(std::thread::current())));
            let mut cx = Context::from_waker(&waker);
            if Pin::new(&mut self.task).poll(&mut cx).is_ready() {
                break;
            }
            std::thread::park_timeout(Duration::from_millis(5));
        }
        Ok(())
    }
    pub fn metrics(&self) -> StreamMetrics {
        self.metrics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn event(
        &self,
        event: Option<Result<Event, antalya_rs::Error>>,
    ) -> Result<Poll, antalya_rs::Error> {
        match event {
            Some(Ok(Event::Schema(schema))) => Ok(Poll::Schema(schema)),
            Some(Ok(Event::Batch(batch))) => {
                let mut m = self.metrics.lock().unwrap_or_else(|e| e.into_inner());
                m.batches = m.batches.saturating_add(1);
                m.rows = m.rows.saturating_add(batch.num_rows() as u64);
                m.charged_bytes = m.charged_bytes.saturating_add(batch.charged_bytes() as u64);
                Ok(Poll::Batch(batch))
            }
            Some(Ok(Event::Complete)) => Ok(Poll::Complete),
            Some(Err(error)) => Err(error),
            None => Err(antalya_rs::Error::Unconfirmed),
        }
    }
    #[cfg(test)]
    pub fn poll(&mut self) -> Result<Poll, antalya_rs::Error> {
        let event = self.rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
        match event {
            Ok(event) => self.event(Some(event)),
            Err(mpsc::error::TryRecvError::Empty) => Ok(Poll::Pending),
            Err(mpsc::error::TryRecvError::Disconnected) => self.terminal_event(),
        }
    }
    fn terminal_event(&mut self) -> Result<Poll, antalya_rs::Error> {
        let result = self
            .terminal
            .try_recv()
            .unwrap_or(Err(antalya_rs::Error::Unconfirmed));
        self.event(Some(result.map(|()| Event::Complete)))
    }
    /// Tokio registers a waker before returning Pending. unpark retains a token,
    /// so an arrival between poll_recv and park cannot be lost. No PostgreSQL
    /// API is called from the worker thread. The timeout bounds interrupt latency.
    pub fn receive(
        &mut self,
        mut check_interrupt: impl FnMut(),
    ) -> Result<Poll, antalya_rs::Error> {
        let waker = Waker::from(Arc::new(BackendWake(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        loop {
            check_interrupt();
            let event = self
                .rx
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .poll_recv(&mut cx);
            match event {
                TaskPoll::Ready(Some(event)) => return self.event(Some(event)),
                TaskPoll::Ready(None) => return self.terminal_event(),
                TaskPoll::Pending => {
                    let start = Instant::now();
                    std::thread::park_timeout(Duration::from_millis(10));
                    let mut m = self.metrics.lock().unwrap_or_else(|e| e.into_inner());
                    m.consumer_waits = m.consumer_waits.saturating_add(1);
                    m.consumer_wait = m.consumer_wait.saturating_add(start.elapsed());
                }
            }
        }
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.close(|| {});
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deadline_reclaims_unread_queue_and_permit_without_consumer_polling() {
        let session = Session::from_options(&HashMap::from([(
            "bearer_token".into(),
            "test-token".into(),
        )]))
        .unwrap();
        let limiter = Arc::new(Semaphore::new(1));
        let permit = limiter.clone().try_acquire_owned().unwrap();
        let (producer, rx, terminal) = Producer::channel(Some(permit));
        let metrics = Arc::new(Mutex::new(StreamMetrics::default()));
        let producer_metrics = metrics.clone();
        let schema = Arc::new(arrow_schema::Schema::empty());
        let retained = Arc::downgrade(&schema);
        let task = session.runtime.spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_millis(30);
            let result = complete_before(deadline, async {
                assert!(
                    send(&producer.tx, Ok(Event::Schema(schema)), &producer_metrics)
                        .await
                        .is_ok()
                );
                std::future::pending().await
            })
            .await;
            producer.finish(result);
        });
        let mut stream = Stream {
            rx,
            terminal,
            task,
            metrics,
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !stream.task.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            stream.task.is_finished(),
            "terminal delivery must not hold the producer"
        );
        assert_eq!(limiter.available_permits(), 1);
        assert!(
            retained.upgrade().is_none(),
            "timeout must discard unread payloads"
        );
        assert!(matches!(
            stream.receive(|| {}),
            Err(antalya_rs::Error::Timeout)
        ));
        assert!(matches!(
            stream.receive(|| {}),
            Err(antalya_rs::Error::Unconfirmed)
        ));
    }

    #[test]
    fn aborted_blocking_jobs_keep_their_four_runtime_slots_until_exit() {
        let session = Session::from_options(&HashMap::from([(
            "bearer_token".into(),
            "test-token".into(),
        )]))
        .unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let mut releases = Vec::new();
        let mut jobs = Vec::new();
        for _ in 0..4 {
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            releases.push(release_tx);
            let started = started_tx.clone();
            jobs.push(session.runtime.spawn_blocking(move || {
                started.send(()).unwrap();
                let _ = release_rx.recv_timeout(Duration::from_secs(5));
            }));
        }
        for _ in 0..4 {
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        for job in &jobs {
            job.abort();
        }
        let (extra_tx, extra_rx) = std::sync::mpsc::channel();
        let extra = session.runtime.spawn_blocking(move || {
            let _ = extra_tx.send(());
        });
        let ran_while_occupied = extra_rx.recv_timeout(Duration::from_millis(100)).is_ok();
        let (cancelled_tx, cancelled_rx) = std::sync::mpsc::channel();
        let queued = session.runtime.spawn_blocking(move || {
            let _ = cancelled_tx.send(());
        });
        queued.abort();
        // Blocking-pool saturation must not stall the async worker itself.
        assert_eq!(
            session
                .runtime
                .block_on(session.runtime.spawn(async { 7 }))
                .unwrap(),
            7
        );
        // Release and join even on the RED path; blocking jobs cannot be aborted.
        for release in releases {
            let _ = release.send(());
        }
        for job in jobs {
            session.runtime.block_on(job).unwrap();
        }
        session.runtime.block_on(extra).unwrap();
        let queued_result = session.runtime.block_on(queued);
        if !ran_while_occupied {
            assert!(queued_result.unwrap_err().is_cancelled());
            assert!(
                cancelled_rx.try_recv().is_err(),
                "cancelled queued work must never run"
            );
        }
        assert!(
            !ran_while_occupied,
            "aborted running work must retain blocking-pool admission"
        );
    }

    #[test]
    fn full_queue_preserves_data_then_exact_terminal_without_holding_admission() {
        for success in [true, false] {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let limiter = Arc::new(Semaphore::new(1));
            let (producer, rx, terminal) =
                Producer::channel(Some(limiter.clone().try_acquire_owned().unwrap()));
            let metrics = Arc::new(Mutex::new(StreamMetrics::default()));
            let producer_metrics = metrics.clone();
            let schema = Arc::new(arrow_schema::Schema::empty());
            let retained = Arc::downgrade(&schema);
            let task = runtime.spawn(async move {
                assert!(
                    send(&producer.tx, Ok(Event::Schema(schema)), &producer_metrics)
                        .await
                        .is_ok()
                );
                producer.finish(if success {
                    Ok(())
                } else {
                    Err(antalya_rs::Error::QueryFailed)
                });
            });
            runtime.block_on(async {
                while !task.is_finished() {
                    tokio::task::yield_now().await;
                }
            });
            assert_eq!(limiter.available_permits(), 1);
            assert!(retained.upgrade().is_some());
            let mut stream = Stream {
                rx,
                terminal,
                task,
                metrics,
            };
            assert!(matches!(stream.receive(|| {}), Ok(Poll::Schema(_))));
            assert!(retained.upgrade().is_none());
            if success {
                assert!(matches!(stream.receive(|| {}), Ok(Poll::Complete)));
            } else {
                assert!(matches!(
                    stream.receive(|| {}),
                    Err(antalya_rs::Error::QueryFailed)
                ));
            }
            assert!(matches!(
                stream.receive(|| {}),
                Err(antalya_rs::Error::Unconfirmed)
            ));
        }
    }

    #[test]
    fn producer_panic_discards_queue_and_fails_closed() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let limiter = Arc::new(Semaphore::new(1));
        let (producer, rx, terminal) =
            Producer::channel(Some(limiter.clone().try_acquire_owned().unwrap()));
        let metrics = Arc::new(Mutex::new(StreamMetrics::default()));
        let producer_metrics = metrics.clone();
        let schema = Arc::new(arrow_schema::Schema::empty());
        let retained = Arc::downgrade(&schema);
        let task = runtime.spawn(async move {
            assert!(
                send(&producer.tx, Ok(Event::Schema(schema)), &producer_metrics)
                    .await
                    .is_ok()
            );
            panic!("injected producer panic");
        });
        runtime.block_on(async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        });
        assert_eq!(limiter.available_permits(), 1);
        assert!(retained.upgrade().is_none());
        let mut stream = Stream {
            rx,
            terminal,
            task,
            metrics,
        };
        assert!(matches!(
            stream.receive(|| {}),
            Err(antalya_rs::Error::Unconfirmed)
        ));
    }

    #[test]
    fn abort_before_first_poll_publishes_unconfirmed_and_releases_admission() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let limiter = Arc::new(Semaphore::new(1));
        let (producer, rx, terminal) =
            Producer::channel(Some(limiter.clone().try_acquire_owned().unwrap()));
        let task = runtime.spawn(async move {
            std::future::pending::<()>().await;
            producer.finish(Ok(()));
        });
        task.abort();
        runtime.block_on(async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        });
        assert_eq!(limiter.available_permits(), 1);
        let mut stream = Stream {
            rx,
            terminal,
            task,
            metrics: Arc::new(Mutex::new(StreamMetrics::default())),
        };
        assert!(matches!(
            stream.receive(|| {}),
            Err(antalya_rs::Error::Unconfirmed)
        ));
    }

    #[test]
    fn receiver_close_before_abort_cannot_confirm_a_blocked_send() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (producer, rx, terminal) = Producer::channel(None);
        let schema = Arc::new(arrow_schema::Schema::empty());
        assert!(
            producer
                .tx
                .try_send(Ok(Event::Schema(schema.clone())))
                .is_ok()
        );
        let metrics = Arc::new(Mutex::new(StreamMetrics::default()));
        let producer_metrics = metrics.clone();
        let task = runtime.spawn(async move {
            let result = async {
                send(&producer.tx, Ok(Event::Schema(schema)), &producer_metrics).await?;
                std::future::pending().await
            }
            .await;
            producer.finish(result);
        });
        runtime.block_on(tokio::task::yield_now());
        assert!(!task.is_finished());
        // Reproduce close's receive-side cleanup, then let the worker observe
        // closure before abort is requested. No network or timing race is needed.
        {
            let mut rx = rx.lock().unwrap();
            rx.close();
            while rx.try_recv().is_ok() {}
        }
        runtime.block_on(async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        });
        let mut stream = Stream {
            rx,
            terminal,
            task,
            metrics,
        };
        assert!(matches!(
            stream.receive(|| {}),
            Err(antalya_rs::Error::Unconfirmed)
        ));
    }

    #[test]
    fn bounded_close_clears_queue_but_keeps_permit_until_task_destruction() {
        // An undriven runtime makes delayed cancellation deterministic.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let limiter = Arc::new(Semaphore::new(1));
        let (producer, rx, terminal) =
            Producer::channel(Some(limiter.clone().try_acquire_owned().unwrap()));
        let schema = Arc::new(arrow_schema::Schema::empty());
        let retained = Arc::downgrade(&schema);
        assert!(producer.tx.try_send(Ok(Event::Schema(schema))).is_ok());
        let task = runtime.spawn(async move {
            std::future::pending::<()>().await;
            producer.finish(Ok(()));
        });
        let mut stream = Stream {
            rx,
            terminal,
            task,
            metrics: Arc::new(Mutex::new(StreamMetrics::default())),
        };
        let started = Instant::now();
        assert!(stream.close(|| {}).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(retained.upgrade().is_none());
        assert_eq!(
            limiter.available_permits(),
            0,
            "abort request is not destruction acknowledgement"
        );
        runtime.block_on(async {
            while !stream.task.is_finished() {
                tokio::task::yield_now().await;
            }
        });
        assert_eq!(limiter.available_permits(), 1);
        assert!(matches!(
            stream.receive(|| {}),
            Err(antalya_rs::Error::Unconfirmed)
        ));
    }

    #[test]
    fn notification_receive_does_not_hide_failure_after_provisional_batch() {
        let (options, fixture) = http_fixture(
            include_bytes!("../../tests/fixtures/one-u64.arrow"),
            br#"{"data":[{"starts":"1","finishes":"0","errors":"1","cancelled":"0","read_rows":"1","read_bytes":"8","result_rows":"1"}]}"#,
        );
        let session = Session::from_options(&options).unwrap();
        let mut stream = session.start("SELECT toUInt64(1) n", None).unwrap();
        assert!(matches!(stream.receive(|| {}), Ok(Poll::Schema(_))));
        let Poll::Batch(batch) = stream.receive(|| {}).unwrap() else {
            panic!("missing provisional batch")
        };
        assert_eq!(batch.num_rows(), 1);
        drop(batch);
        let err = stream.receive(|| {}).err().unwrap();
        assert_eq!(err.kind(), antalya_rs::ErrorKind::QueryFailed);
        fixture.join().unwrap();
    }
    #[test]
    fn notification_receive_handles_queued_events_disconnect_and_interrupts() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (producer, rx, terminal) = Producer::channel(None);
        let task = runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            producer.finish(Ok(()));
        });
        let mut stream = Stream {
            rx,
            terminal,
            task,
            metrics: Arc::new(Mutex::new(StreamMetrics::default())),
        };
        let mut interrupts = 0;
        assert!(matches!(
            stream.receive(|| interrupts += 1),
            Ok(Poll::Complete)
        ));
        assert!(interrupts >= 1);
        assert!(matches!(
            stream.receive(|| {}),
            Err(antalya_rs::Error::Unconfirmed)
        ));
    }
    #[test]
    fn block_size_setting_is_bounded() {
        for value in ["1", "1024", "4096", "16384"] {
            assert!(
                make_query("SELECT 1", Some(&format!("max_block_size={value}"))).is_ok(),
                "valid bounded block size: {value}"
            );
        }
        for value in [
            "0",
            "16385",
            "-1",
            "1.5",
            "NaN",
            "1; SELECT 2",
            "18446744073709551616",
        ] {
            assert!(make_query("SELECT 1", Some(&format!("max_block_size={value}"))).is_err());
        }
        assert!(
            make_query(
                "SELECT 1",
                Some("max_block_size=4096,max_result_bytes=999999999")
            )
            .is_err()
        );
        assert!(make_query("SELECT 1", Some("max_block_size=4096,log_queries=0")).is_err());
    }
    #[test]
    fn auth_and_settings_fail_closed() {
        let mut opts = HashMap::new();
        assert!(Session::from_options(&opts).is_err());
        opts.insert("password".into(), "sentinel-secret".into());
        let err = Session::from_options(&opts).err().unwrap();
        assert!(err.contains("legacy"));
        assert!(!err.contains("sentinel-secret"));
        for bad in [
            "password=secret",
            "max_threads=1,log_queries=0",
            "max_threads=1; SELECT 2",
            "malformed",
        ] {
            // Numeric values must be validated before transport too.
            assert!(make_query("SELECT 1", Some(bad)).is_err(), "{bad}");
        }
    }
    #[test]
    fn cache_and_concurrency_are_bounded() {
        let mut opts = HashMap::from([("bearer_token".into(), "token".into())]);
        let first = Session::from_options(&opts).unwrap();
        for port in 8123..8130 {
            opts.insert("url".into(), format!("http://127.0.0.1:{port}"));
            let session = Session::from_options(&opts).unwrap();
            assert!(Arc::ptr_eq(&first.runtime, &session.runtime));
        }
        CACHE.with(|cache| assert_eq!(cache.borrow().as_ref().unwrap().clients.len(), CACHE_CAP));
        let _permits = first.limiter.try_acquire_many(4).unwrap();
        assert!(first.start("SELECT 1", None).is_err());
    }
    #[test]
    fn provisional_batch_retains_budget_and_never_hides_failure_or_disconnect() {
        // Generated locally with pyarrow: one UInt64 column named n, one row containing 1.
        let (options, fixture) = http_fixture(
            include_bytes!("../../tests/fixtures/one-u64.arrow"),
            br#"{"data":[{"starts":"1","finishes":"0","errors":"1","cancelled":"0","read_rows":"1","read_bytes":"8","result_rows":"1"}]}"#,
        );
        let mut session = Session::from_options(&options).unwrap();
        let config = Config::new(
            Endpoint::parse(&options["url"]).unwrap(),
            Credentials::bearer(&options["bearer_token"]).unwrap(),
        )
        .timeout(Duration::from_secs(3))
        .resources(antalya_rs::ResourceLimits {
            max_retained_bytes: 1024,
            ..Default::default()
        })
        .execution_limits(antalya_rs::ExecutionLimits {
            max_batch_bytes: 1024,
            ..Default::default()
        });
        session.client = {
            let _guard = session.runtime.enter();
            AntalyaClient::new(config).unwrap()
        };
        let mut stream = session.start("SELECT toUInt64(1) n", None).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            assert!(Instant::now() < deadline);
            match stream.poll().unwrap() {
                Poll::Pending => std::thread::sleep(Duration::from_millis(2)),
                Poll::Schema(_) => break,
                _ => panic!("schema must precede the provisional batch"),
            }
        }
        while stream.rx.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        // The queued batch retains capacity: the worker cannot reserve its next poll.
        std::thread::sleep(Duration::from_millis(30));
        assert!(!stream.task.is_finished());
        let Poll::Batch(batch) = stream.poll().unwrap() else {
            panic!("expected provisional batch");
        };
        assert_eq!(batch.num_rows(), 1);
        assert!(batch.charged_bytes() > 0);
        let retained = batch.clone();
        drop(batch);
        std::thread::sleep(Duration::from_millis(30));
        assert!(matches!(stream.poll(), Ok(Poll::Pending)));
        assert!(
            !stream.task.is_finished(),
            "a retained clone holds capacity"
        );
        drop(retained);
        loop {
            assert!(Instant::now() < deadline);
            match stream.poll() {
                Ok(Poll::Pending) => std::thread::sleep(Duration::from_millis(2)),
                Err(error) => {
                    assert_eq!(error.kind(), antalya_rs::ErrorKind::QueryFailed);
                    break;
                }
                _ => panic!("provisional data must not hide terminal failure"),
            }
        }
        while !stream.task.is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(matches!(stream.poll(), Err(antalya_rs::Error::Unconfirmed)));
        fixture.join().unwrap();
    }
    #[test]
    fn dropped_scan_acknowledges_task_cancellation_before_replacement() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let limiter = Arc::new(Semaphore::new(4));
        let _other_scans = limiter.try_acquire_many(3).unwrap();
        let permit = limiter.clone().try_acquire_owned().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let task = runtime.spawn(async move {
            let _permit = permit;
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel();
        let (gated_tx, gated_rx) = std::sync::mpsc::channel();
        // Intentionally block the only worker to make asynchronous abort's race deterministic.
        let blocker = runtime.spawn(async move {
            gated_tx.send(()).unwrap();
            gate_rx.recv().unwrap();
        });
        gated_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let (_producer, rx, terminal) = Producer::channel(None);
        let stream = Stream {
            rx,
            terminal,
            task,
            metrics: Arc::new(Mutex::new(StreamMetrics::default())),
        };
        let (dropping_tx, dropping_rx) = std::sync::mpsc::channel();
        let dropper = std::thread::spawn(move || {
            dropping_tx.send(()).unwrap();
            drop(stream);
        });
        dropping_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let waited_for_drop = !dropper.is_finished();
        let occupied_while_gated = limiter.available_permits();
        gate_tx.send(()).unwrap();
        dropper.join().unwrap();
        assert!(
            waited_for_drop,
            "cleanup must await actual task destruction"
        );
        assert_eq!(occupied_while_gated, 0, "dying worker retains capacity");
        assert!(
            limiter.clone().try_acquire_owned().is_ok(),
            "replacement can start immediately"
        );
        runtime.block_on(blocker).unwrap();
    }
    fn http_fixture(
        arrow: &'static [u8],
        terminal: &'static [u8],
    ) -> (HashMap<String, String>, std::thread::JoinHandle<()>) {
        http_fixture_block_size(arrow, terminal, None)
    }
    fn http_fixture_block_size(
        arrow: &'static [u8],
        terminal: &'static [u8],
        expected_block_size: Option<&'static str>,
    ) -> (HashMap<String, String>, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let fixture = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            for index in 0..3 {
                let mut socket = loop {
                    if let Ok((socket, _)) = listener.accept() {
                        break socket;
                    }
                    assert!(Instant::now() < deadline, "fixture request deadline");
                    std::thread::sleep(Duration::from_millis(2));
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0u8; 4096];
                    let n = socket.read(&mut chunk).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&chunk[..n]);
                    if let Some(end) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&request[..end])
                            .unwrap()
                            .to_ascii_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        assert!(headers.contains("authorization: bearer fixture-test"));
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                if index == 0
                    && let Some(size) = expected_block_size
                {
                    let request = std::str::from_utf8(&request).unwrap();
                    assert!(
                        request.contains(&format!("max_block_size={size}")),
                        "block size must reach the data request"
                    );
                    assert!(request.contains("max_result_rows=1000000"));
                    assert!(request.contains("max_result_bytes=67108864"));
                    assert!(request.contains("result_overflow_mode=throw"));
                }
                let body: &[u8] = match index {
                    0 => arrow,
                    1 => b"",
                    _ => terminal,
                };
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                socket.write_all(body).unwrap();
            }
        });
        let options = HashMap::from([
            ("url".into(), format!("http://{address}")),
            ("bearer_token".into(), "fixture-test".into()),
            ("timeout_ms".into(), "3000".into()),
        ]);
        (options, fixture)
    }
    #[test]
    fn block_size_reaches_transport_without_changing_safety_limits() {
        // Check acceptance before starting a server, including during the RED run.
        assert!(make_query("SELECT 1", Some("max_block_size=4096")).is_ok());
        for (setting, expected) in [(None, "1024"), (Some("max_block_size=4096"), "4096")] {
            let (options, fixture) = http_fixture_block_size(
                include_bytes!("../../tests/fixtures/empty-u64.arrow"),
                br#"{"data":[{"starts":"1","finishes":"1","errors":"0","cancelled":"0","read_rows":"0","read_bytes":"0","result_rows":"0"}]}"#,
                Some(expected),
            );
            let session = Session::from_options(&options).unwrap();
            let mut stream = session
                .start("SELECT toUInt64(1) n WHERE 0", setting)
                .unwrap();
            loop {
                match stream.receive(|| {}).unwrap() {
                    Poll::Complete => break,
                    Poll::Schema(_) => {}
                    _ => panic!("expected schema-only confirmed result"),
                }
            }
            fixture.join().unwrap();
        }
    }
    #[test]
    fn schema_only_http_stream_delivers_schema_before_confirmed_eof() {
        let (options, fixture) = http_fixture(
            include_bytes!("../../tests/fixtures/empty-u64.arrow"),
            br#"{"data":[{"starts":"1","finishes":"1","errors":"0","cancelled":"0","read_rows":"0","read_bytes":"0","result_rows":"0"}]}"#,
        );
        let session = Session::from_options(&options).unwrap();
        let mut stream = session.start("SELECT toUInt64(1) n WHERE 0", None).unwrap();
        let mut schema_seen = false;
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            assert!(Instant::now() < deadline);
            match stream.poll().unwrap() {
                Poll::Pending => std::thread::sleep(Duration::from_millis(2)),
                Poll::Schema(schema) => {
                    assert!(!schema_seen);
                    assert!(
                        super::super::values::validate_schema(
                            &schema,
                            &[super::super::values::Target::Int64]
                        )
                        .is_ok()
                    );
                    assert!(
                        super::super::values::validate_schema(
                            &schema,
                            &[super::super::values::Target::Text]
                        )
                        .is_err()
                    );
                    assert!(
                        super::super::values::validate_schema(
                            &schema,
                            &[
                                super::super::values::Target::Int64,
                                super::super::values::Target::Int64
                            ]
                        )
                        .is_err()
                    );
                    schema_seen = true;
                }
                Poll::Batch(_) => panic!("schema-only fixture has no batches"),
                Poll::Complete => {
                    assert!(schema_seen);
                    break;
                }
            }
        }
        fixture.join().unwrap();
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    #[test]
    fn bounded_batch_configuration() {
        let mut options = HashMap::new();
        assert_eq!(batch_bytes(&options), Ok(MAX_BATCH_BYTES));
        for bad in ["0", "65535", "16777217", "-1", "garbage"] {
            options.insert("batch_bytes".into(), bad.into());
            assert!(batch_bytes(&options).is_err());
        }
        options.insert("batch_bytes".into(), "1048576".into());
        assert_eq!(batch_bytes(&options), Ok(1048576));
    }
    #[test]
    fn rescan_offsets_are_not_summed_and_peak_is_maximum() {
        let one = StreamMetrics {
            executions: 1,
            last_query_id: "one".into(),
            peak_batch_bytes: 12,
            execution: Some(antalya_rs::ExecutionMetrics {
                first_batch: Some(Duration::from_millis(3)),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut total = StreamMetrics::default();
        total.accumulate(&one);
        assert_eq!(
            total.execution.as_ref().unwrap().first_batch,
            Some(Duration::from_millis(3))
        );
        total.accumulate(&StreamMetrics {
            last_query_id: "two".into(),
            peak_batch_bytes: 8,
            ..one
        });
        assert_eq!(total.peak_batch_bytes, 12);
        assert_eq!(total.last_query_id, "two");
        assert_eq!(total.executions, 2);
        assert!(total.execution.unwrap().first_batch.is_none());
    }
    #[test]
    fn unfinished_rescan_clears_first_batch_offset() {
        let mut total = StreamMetrics::default();
        total.accumulate(&StreamMetrics {
            executions: 1,
            execution: Some(antalya_rs::ExecutionMetrics {
                first_batch: Some(Duration::from_millis(3)),
                ..Default::default()
            }),
            ..Default::default()
        });
        total.accumulate(&StreamMetrics {
            executions: 1,
            last_query_id: "started-but-incomplete".into(),
            ..Default::default()
        });
        assert_eq!(total.executions, 2);
        assert_eq!(total.last_query_id, "started-but-incomplete");
        assert!(total.execution.unwrap().first_batch.is_none());
    }
}
