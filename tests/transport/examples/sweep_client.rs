// SPDX-License-Identifier: Apache-2.0
//! Persistent bounded sweep executor for an explicitly owned benchmark database.
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]
#[path = "sweep_client/plan.rs"]
mod plan;
#[path = "sweep_client/rows.rs"]
mod rows;
#[path = "sweep_client/run.rs"]
mod run;
use antalya_rs::{
    AntalyaClient, Config, Credentials, Endpoint, ExecutionLimits, PoolOptions, ResourceLimits,
    TerminalPolicy, TerminalStrategy,
};
use serde_json::json;
use std::{
    io::{self, BufRead, Write},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    Client(antalya_rs::Error),
    Io(io::Error),
    Json(serde_json::Error),
    Join(tokio::task::JoinError),
    Deadline,
    Cancelled,
    Unconfirmed,
    Cleanup {
        operation: Option<Box<Error>>,
        cleanup: Box<Error>,
    },
}
impl Error {
    fn fatal(&self) -> bool {
        matches!(self, Self::Cleanup { .. } | Self::Join(_))
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "invalid sweep: {s}"),
            Self::Client(e) => write!(f, "client: {e}"),
            Self::Io(_) => f.write_str("worker I/O failed"),
            Self::Json(_) => f.write_str("invalid command JSON"),
            Self::Join(_) => f.write_str("worker task failed"),
            Self::Deadline => f.write_str("sweep deadline exceeded"),
            Self::Cancelled => f.write_str("sweep cancelled"),
            Self::Unconfirmed => f.write_str("execution did not succeed"),
            Self::Cleanup { .. } => {
                f.write_str("cleanup unconfirmed; discard worker and owned fixture")
            }
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            Self::Join(e) => Some(e),
            Self::Cleanup { operation, cleanup } => {
                Some(operation.as_deref().unwrap_or(cleanup.as_ref()))
            }
            _ => None,
        }
    }
}
impl From<antalya_rs::Error> for Error {
    fn from(e: antalya_rs::Error) -> Self {
        Self::Client(e)
    }
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl From<tokio::task::JoinError> for Error {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Join(e)
    }
}
fn client(strategy: TerminalStrategy) -> Result<AntalyaClient, Error> {
    let endpoint =
        std::env::var("ANTALYA_TEST_URL").map_err(|_| Error::Invalid("endpoint environment"))?;
    let bearer = std::env::var("ANTALYA_TEST_TOKEN")
        .map_err(|_| Error::Invalid("credential environment"))?;
    Ok(AntalyaClient::new(
        Config::new(Endpoint::parse(&endpoint)?, Credentials::bearer(&bearer)?)
            .timeout(Duration::from_secs(65))
            .pool(PoolOptions {
                max_idle_per_host: 4,
                idle_timeout: Duration::from_secs(120),
            })
            .resources(ResourceLimits {
                max_active_queries: 4,
                max_queued_queries: 0,
                max_control_queries: 4,
                max_retained_bytes: 64 << 20,
                ..Default::default()
            })
            .execution_limits(ExecutionLimits {
                max_rows: 100_000,
                max_batch_bytes: 16 << 20,
                max_total_bytes: 64 << 20,
                ..Default::default()
            })
            .terminal_policy(TerminalPolicy {
                strategy,
                ..Default::default()
            }),
    )?)
}
fn command(input: &mut impl BufRead) -> Result<Option<Vec<u8>>, Error> {
    let mut bytes = Vec::new();
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok((!bytes.is_empty()).then_some(bytes));
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |n| n + 1);
        if bytes.len().saturating_add(count) > 4 << 20 {
            return Err(Error::Invalid("command byte limit"));
        }
        let done = available[count - 1] == b'\n';
        bytes.extend_from_slice(&available[..count]);
        input.consume(count);
        if done {
            return Ok(Some(bytes));
        }
    }
}
fn main() -> Result<(), Error> {
    let (policy, control_policy) = match std::env::args().nth(1).as_deref() {
        None | Some("flush_first") => (TerminalStrategy::FlushFirst, TerminalStrategy::FlushFirst),
        Some("in_band") => (TerminalStrategy::InBand, TerminalStrategy::FlushFirst),
        Some("in_band_observe") => (TerminalStrategy::InBand, TerminalStrategy::ObserveFirst),
        _ => return Err(Error::Invalid("terminal policy")),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    let start = Instant::now();
    let (data, control) = {
        let _guard = runtime.enter();
        (client(policy)?, client(control_policy)?)
    };
    println!(
        "{}",
        json!({"ready":true,"client":"antalya-rs 0.1.1","policy":format!("{policy:?}"),"control_policy":format!("{control_policy:?}"),"runtime_threads":1,"client_setup_ms":start.elapsed().as_secs_f64()*1000.})
    );
    io::stdout().flush()?;
    let mut input = io::stdin().lock();
    while let Some(bytes) = command(&mut input)? {
        let result = serde_json::from_slice::<plan::Plan>(&bytes)
            .map_err(Error::from)
            .and_then(|p| runtime.block_on(run::execute(&data, &control, p)));
        let fatal = result.as_ref().err().is_some_and(Error::fatal);
        let value = match result {
            Ok(v) => v,
            Err(e) => json!({"error":e.to_string(),"cleanup_confirmed":!e.fatal()}),
        };
        println!("{value}");
        io::stdout().flush()?;
        if fatal {
            break;
        }
    }
    Ok(())
}
