// SPDX-License-Identifier: Apache-2.0
use super::{
    Error,
    plan::{self, Plan, TABLE},
    rows::{self, Pair},
};
use antalya_rs::{AntalyaClient, ExecutionMetrics, Outcome, Query};
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::AtomicUsize},
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinSet};

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}
fn metrics(
    id: String,
    kind: &str,
    m: ExecutionMetrics,
    conversion: Duration,
    peak: usize,
) -> Value {
    json!({"query_id":id,"kind":kind,"queued_ms":ms(m.queued),"identity_ms":ms(m.identity),"data_poll_ms":ms(m.data_poll),
        "first_batch_ms":m.first_batch.map(ms),"stream_end_ms":m.stream_end.map(ms),"terminal_ms":ms(m.terminal),"total_ms":ms(m.total),
        "body_wait_ms":m.transfer.map(ms),"decode_ms":m.decode.map(ms),"framing_ms":ms(m.framing),"wire_bytes":m.wire_bytes,
        "control_requests":m.control_requests,"flush_attempts":m.flush_attempts,"flush_ms":ms(m.flush_time),"observe_ms":ms(m.observe_time),
        "poll_sleeps":m.poll_sleeps,"poll_wait_ms":ms(m.poll_wait),"conversion_ms":ms(conversion),"peak_individual_batch_bytes":peak})
}
#[derive(Clone)]
struct Context {
    client: AntalyaClient,
    database: String,
    prefix: String,
    deadline: tokio::time::Instant,
    total: Arc<AtomicUsize>,
}
impl Context {
    fn query(&self, sql: &str, workers: usize, cleanup: bool) -> Result<Query, Error> {
        let remaining = if cleanup {
            Duration::from_secs(60)
        } else {
            self.deadline
                .checked_duration_since(tokio::time::Instant::now())
                .ok_or(Error::Deadline)?
        };
        let mut q = Query::new(
            sql,
            format!("{}{}", self.prefix, uuid::Uuid::new_v4().simple()),
        )?
        .database(&self.database)?;
        for (key, value) in [
            ("max_threads", (4 / workers).to_string()),
            (
                "max_memory_usage",
                (4_000_000_000u64 / workers as u64).to_string(),
            ),
            (
                "max_execution_time",
                remaining.as_secs_f64().ceil().max(1.).to_string(),
            ),
        ] {
            q = q.setting(key, value)?;
        }
        for (key, value) in [
            ("max_block_size", "8192"),
            ("max_insert_block_size", "8192"),
            ("min_insert_block_size_rows", "8192"),
            ("min_insert_block_size_bytes", "0"),
            ("max_ast_elements", "50000"),
            ("max_expanded_ast_elements", "500000"),
            ("max_query_size", "262144"),
            ("short_circuit_function_evaluation", "enable"),
            ("max_result_rows", "100000"),
            ("max_result_bytes", "67108864"),
            ("result_overflow_mode", "throw"),
            ("use_query_cache", "0"),
            ("use_query_condition_cache", "0"),
            ("output_format_arrow_string_as_string", "0"),
            ("output_format_arrow_low_cardinality_as_dictionary", "0"),
            ("cancel_http_readonly_queries_on_client_close", "1"),
        ] {
            q = q.setting(key, value)?;
        }
        Ok(q)
    }
}
struct Reply {
    pairs: Vec<Pair>,
    counts: Option<(u64, u64)>,
    metric: Value,
}
async fn read(
    ctx: Context,
    sql: String,
    workers: usize,
    count: bool,
    mut stop: watch::Receiver<bool>,
) -> Result<Reply, Error> {
    if *stop.borrow() {
        return Err(Error::Cancelled);
    }
    let query = ctx.query(&sql, workers, false)?;
    // Admission happens before dispatch. Cancellation of this wait starts no query.
    let mut execution = tokio::select! {
        _=stop.changed()=>return Err(Error::Cancelled),
        result=tokio::time::timeout_at(ctx.deadline,ctx.client.fetch_arrow(query))=>result.map_err(|_|Error::Deadline)??,
    };
    let mut pairs = Vec::new();
    let mut counts = None;
    let mut conversion = Duration::ZERO;
    let mut peak = 0;
    let result = {
        let consume = async {
            while let Some(batch) = execution.next_batch().await? {
                peak = peak.max(batch.charged_bytes());
                let start = Instant::now();
                if count {
                    if counts.is_some() {
                        return Err(Error::Invalid("multiple count batches"));
                    }
                    counts = Some(rows::counts(&batch)?);
                } else {
                    rows::append(&batch, &mut pairs, &ctx.total)?;
                }
                conversion += start.elapsed();
            }
            if execution.outcome() != Outcome::Succeeded {
                return Err(Error::Unconfirmed);
            }
            if count && counts.is_none() {
                return Err(Error::Invalid("missing candidate counts"));
            }
            Ok(())
        };
        tokio::select! {
            _=stop.changed()=>Err(Error::Cancelled),
            result=tokio::time::timeout_at(ctx.deadline,consume)=>result.map_err(|_|Error::Deadline).and_then(|r|r),
        }
    };
    if let Err(error) = result {
        // Cancel and establish termination, including a race where the server
        // finished after results were discarded. Never turn that race into success.
        if execution.outcome() != Outcome::Succeeded
            && execution.cancel().await.is_err()
            && let Err(cleanup) = execution.confirm_termination().await
        {
            return Err(Error::Cleanup {
                operation: Some(Box::new(error)),
                cleanup: Box::new(Error::Client(cleanup)),
            });
        }
        return Err(error);
    }
    Ok(Reply {
        pairs,
        counts,
        metric: metrics(
            execution.query_id().into(),
            if count { "counts" } else { "pairs" },
            execution.metrics(),
            conversion,
            peak,
        ),
    })
}
async fn command(
    ctx: &Context,
    client: &AntalyaClient,
    sql: &str,
    kind: &str,
    cleanup: bool,
) -> Result<Value, Error> {
    let q = ctx.query(sql, 1, cleanup)?;
    let id = q.query_id().to_owned();
    let report = client
        .execute_command(q)
        .await
        .map_err(|e| Error::Cleanup {
            operation: None,
            cleanup: Box::new(Error::Client(e)),
        })?;
    match report.outcome {
        Outcome::Succeeded => Ok(metrics(id, kind, report.metrics, Duration::ZERO, 0)),
        Outcome::Failed | Outcome::Cancelled => Err(Error::Unconfirmed),
        _ => Err(Error::Cleanup {
            operation: None,
            cleanup: Box::new(Error::Unconfirmed),
        }),
    }
}
async fn batches(
    ctx: &Context,
    queries: &[String],
    workers: usize,
    operations: &mut Vec<Value>,
) -> Result<Vec<Pair>, Error> {
    let (stop, receiver) = watch::channel(false);
    let mut tasks = JoinSet::new();
    let mut next = queries.iter();
    for sql in next.by_ref().take(workers) {
        tasks.spawn(read(
            ctx.clone(),
            sql.clone(),
            workers,
            false,
            receiver.clone(),
        ));
    }
    let mut pairs = Vec::new();
    let mut error = None;
    while let Some(result) = tasks.join_next().await {
        match result.map_err(Error::from).and_then(|r| r) {
            Ok(mut reply) => {
                pairs.append(&mut reply.pairs);
                operations.push(reply.metric);
            }
            Err(e) => {
                stop.send_replace(true);
                if error.is_none() || e.fatal() {
                    error = Some(e);
                }
            }
        }
        if error.is_none()
            && let Some(sql) = next.next()
        {
            tasks.spawn(read(
                ctx.clone(),
                sql.clone(),
                workers,
                false,
                receiver.clone(),
            ));
        }
    }
    // JoinSet is empty here on every normal error path; no detached work.
    match error {
        Some(e) => Err(e),
        None => Ok(pairs),
    }
}
pub async fn execute(
    client: &AntalyaClient,
    control: &AntalyaClient,
    plan: Plan,
) -> Result<Value, Error> {
    plan.validate()?;
    let start = Instant::now();
    let ctx = Context {
        client: client.clone(),
        database: plan.database.clone(),
        prefix: format!(
            "comparison-direct-{}-{}-",
            plan.database.trim_start_matches("wc_benchmark_"),
            uuid::Uuid::new_v4().simple()
        ),
        deadline: tokio::time::Instant::now() + Duration::from_millis(plan.deadline_ms),
        total: Arc::new(AtomicUsize::new(0)),
    };
    let (_stop, receiver) = watch::channel(false);
    let mut operations = Vec::new();
    let mut created = false;
    let mut counts = None;
    let mut refused = false;
    let body = async {
        let mut queries = &plan.queries;
        if let Some(staged) = &plan.staged {
            let preflight = read(
                ctx.clone(),
                format!(
                    "SELECT count(),sum(length(v)) FROM ({})",
                    staged.candidate_select
                ),
                1,
                true,
                receiver.clone(),
            )
            .await?;
            counts = preflight.counts;
            operations.push(preflight.metric);
            let (rows, bytes) = counts.ok_or(Error::Invalid("candidate counts"))?;
            if !plan::allows(plan.source_rows, rows, bytes) {
                refused = true;
            } else if rows == 0 {
                return Ok(Vec::new());
            } else {
                created = true;
                operations.push(
                    command(
                        &ctx,
                        control,
                        &format!(
                            "CREATE TABLE {TABLE} ENGINE=Memory AS {}",
                            staged.candidate_select
                        ),
                        "create",
                        false,
                    )
                    .await?,
                );
                let actual = read(
                    ctx.clone(),
                    format!("SELECT count(),sum(length(v)) FROM {TABLE}"),
                    1,
                    true,
                    receiver.clone(),
                )
                .await?;
                if actual.counts != counts {
                    return Err(Error::Invalid("immutable candidate population changed"));
                }
                operations.push(actual.metric);
                queries = &staged.queries;
            }
        }
        batches(&ctx, queries, plan.workers, &mut operations).await
    }
    .await;
    if created && !body.as_ref().err().is_some_and(Error::fatal) {
        match command(
            &ctx,
            control,
            &format!("DROP TABLE IF EXISTS {TABLE} SYNC"),
            "drop",
            true,
        )
        .await
        {
            Ok(metric) => operations.push(metric),
            Err(cleanup) => {
                return Err(Error::Cleanup {
                    operation: body.err().map(Box::new),
                    cleanup: Box::new(cleanup),
                });
            }
        }
    }
    let pairs = body?;
    let elapsed = start.elapsed();
    if elapsed > Duration::from_millis(plan.deadline_ms) {
        return Err(Error::Deadline);
    }
    Ok(
        json!({"request_ms":ms(elapsed),"pairs":pairs,"executions":operations,"candidate_rows":counts.map(|c|c.0),
        "candidate_value_bytes":counts.map(|c|c.1),"staging_refused":refused,"max_workers":plan.workers,"cleanup_confirmed":true}),
    )
}
