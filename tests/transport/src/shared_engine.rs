// SPDX-License-Identifier: Apache-2.0
//! Current Antalya client, bounded Arrow consumption and verified result timing.
use super::{CLOCK_BOUNDARY, Error, Result, finish_verified, uint};
use antalya_rs::{
    AntalyaClient, Cause, Config, Credentials, Endpoint, ExecutionLimits, Outcome, Phase, Query,
    ResourceLimits, TerminalPolicy, TerminalStrategy,
};
use arrow_array::{Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Schema};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

const CAP: usize = 16 * 1024 * 1024;
const ROWS: usize = 32768;

fn native_refusal(error: antalya_rs::Error, outcome: Outcome) -> Option<(&'static str, u32)> {
    // A data exception alone is not terminal proof. next_batch must already
    // have reconciled it with the private marker under the original deadline.
    if outcome != Outcome::Failed || error.phase() != Some(Phase::Data) {
        return None;
    }
    match error.cause()? {
        Cause::ServerResponse(Some(159)) => Some(("time", 159)),
        Cause::ServerResponse(Some(241)) => Some(("memory", 241)),
        _ => None,
    }
}

pub(super) fn client(url: &str, token: &str) -> Result<AntalyaClient> {
    client_with_policy(url, token, TerminalStrategy::FlushFirst)
}

pub(super) fn client_with_policy(
    url: &str,
    token: &str,
    strategy: TerminalStrategy,
) -> Result<AntalyaClient> {
    AntalyaClient::new(
        Config::new(
            Endpoint::parse(url).map_err(Error::Shared)?,
            Credentials::bearer(token).map_err(Error::Shared)?,
        )
        .timeout(Duration::from_secs(30))
        .terminal_policy(TerminalPolicy {
            strategy,
            ..Default::default()
        })
        .resources(ResourceLimits {
            max_active_queries: 1,
            max_queued_queries: 0,
            max_control_queries: 1,
            max_export_tasks: 1,
            max_retained_bytes: CAP,
        })
        .execution_limits(ExecutionLimits {
            max_rows: ROWS as u64,
            max_batches: 1024,
            max_total_bytes: CAP as u64,
            max_batch_bytes: CAP,
        }),
    )
    .map_err(Error::Shared)
}

fn schema(schema: &Schema, count: bool) -> Result<()> {
    let fields = schema.fields();
    if fields.len() != if count { 1 } else { 2 }
        || fields[0].name() != "id"
        || fields[0].data_type() != &DataType::UInt64
        || (!count && (fields[1].name() != "value" || fields[1].data_type() != &DataType::Utf8))
    {
        return Err(Error::Values);
    }
    Ok(())
}

struct ArrowDigest {
    hash: Sha256,
    count: bool,
    expected: u64,
    rows: usize,
    previous: Option<u64>,
}
impl ArrowDigest {
    fn new(count: bool, expected: u64) -> Self {
        Self {
            hash: Sha256::new(),
            count,
            expected,
            rows: 0,
            previous: None,
        }
    }
    fn push(&mut self, batch: &RecordBatch) -> Result<()> {
        schema(&batch.schema(), self.count)?;
        if batch.num_rows() > ROWS.saturating_sub(self.rows) {
            return Err(Error::Limit);
        }
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or(Error::Values)?;
        let values = if self.count {
            None
        } else {
            Some(
                batch
                    .column(1)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or(Error::Values)?,
            )
        };
        for row in 0..batch.num_rows() {
            if ids.is_null(row) {
                return Err(Error::Values);
            }
            let id = ids.value(row);
            if self.count {
                if self.rows != 0 || id != self.expected {
                    return Err(Error::Values);
                }
            } else if self.previous.is_some_and(|previous| previous >= id) {
                return Err(Error::Values);
            }
            self.hash.update(id.to_le_bytes());
            if let Some(values) = values {
                if values.is_null(row) {
                    return Err(Error::Values);
                }
                let value = values.value(row);
                if value.len() > 8192 {
                    return Err(Error::Limit);
                }
                self.hash.update([1]);
                self.hash.update((value.len() as u64).to_le_bytes());
                self.hash.update(value.as_bytes());
            }
            self.previous = Some(id);
            self.rows += 1;
        }
        Ok(())
    }
    fn finish(self, outcome: Outcome) -> Result<(usize, String)> {
        if outcome != Outcome::Succeeded {
            return Err(Error::Incomplete);
        }
        if self.count && self.rows != 1 {
            return Err(Error::Values);
        }
        Ok((self.rows, format!("{:x}", self.hash.finalize())))
    }
}

/// EXPLAIN uses the same authenticated terminal policy as measured data.
/// Only a single bounded Utf8 cell is accepted; no arbitrary result is blessed.
fn explain_cell(batch: &RecordBatch) -> Result<Option<String>> {
    if batch.num_columns() != 1 || batch.column(0).data_type() != &DataType::Utf8 {
        return Err(Error::Values);
    }
    if batch.num_rows() == 0 {
        return Ok(None);
    }
    if batch.num_rows() != 1 {
        return Err(Error::Values);
    }
    let values = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or(Error::Values)?;
    if values.is_null(0) || values.value(0).len() > 262144 {
        return Err(Error::Values);
    }
    Ok(Some(values.value(0).to_owned()))
}

pub(super) async fn explain(client: &AntalyaClient, command: &Value) -> Result<Value> {
    if policy(command)? != "in_band" || command["engine"] != "antalya_shared" {
        return Err(Error::Input);
    }
    let sql = command["sql"].as_str().ok_or(Error::Input)?;
    if !sql.starts_with("SELECT ") || sql.len() > 65536 {
        return Err(Error::Input);
    }
    let start = Instant::now();
    let id = uuid::Uuid::new_v4().to_string();
    let mut query =
        Query::new(format!("EXPLAIN json=1, indexes=1 {sql}"), &id).map_err(Error::Shared)?;
    for (key, value) in command["settings"].as_object().ok_or(Error::Input)? {
        query = query
            .setting(
                key,
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            )
            .map_err(Error::Shared)?;
    }
    query = query
        .setting("output_format_arrow_string_as_string", "1")
        .map_err(Error::Shared)?
        .setting("max_result_rows", "16")
        .map_err(Error::Shared)?
        .setting("max_result_bytes", "1048576")
        .map_err(Error::Shared)?;
    let mut execution = client.fetch_arrow(query).await.map_err(Error::Shared)?;
    let mut plan = None;
    let mut batches = 0;
    let mut bytes = 0;
    while let Some(batch) = execution.next_batch().await.map_err(Error::Shared)? {
        batches += 1;
        bytes += batch.charged_bytes();
        if batches > 64 || bytes > 1048576 {
            return Err(Error::Limit);
        }
        if let Some(value) = explain_cell(&batch)?
            && plan.replace(value).is_some()
        {
            return Err(Error::Values);
        }
    }
    if execution.outcome() != Outcome::Succeeded {
        return Err(Error::Incomplete);
    }
    let plan = plan.ok_or(Error::Values)?;
    let m = execution.metrics();
    Ok(
        json!({"query_id":id,"plan_json":plan,"completion":"Succeeded",
        "terminal_policy":"InBand(explicit)","control_requests":m.control_requests,
        "client_ms":start.elapsed().as_secs_f64()*1000.0,
        "http_body_bytes":m.wire_bytes,
        "terminal_ms":m.terminal.as_secs_f64()*1000.0,
        "clock_boundary":"explain-request-through-terminal-proof-v1"}),
    )
}

pub(super) async fn measure(client: &AntalyaClient, command: &Value) -> Result<Value> {
    let count = command["mode"] == "count";
    let optimistic = command["engine"] == "antalya_shared_optimistic";
    let terminal_policy = if policy(command)? == "in_band" {
        "InBand(explicit)"
    } else {
        "FlushFirst(default)"
    };
    let client_path = if optimistic {
        "AntalyaClient::fetch_arrow/next_provisional_batch/confirm_completion"
    } else {
        "AntalyaClient::fetch_arrow/next_batch"
    };
    let id = uuid::Uuid::new_v4().to_string();
    let start = Instant::now();
    let expected = uint(&command["expected_total"])?;
    let expected_rows = if optimistic {
        let rows = uint(&command["expected_rows"])?;
        if rows > ROWS as u64 {
            return Err(Error::Limit);
        }
        Some(rows as usize)
    } else {
        None
    };
    let mut query =
        Query::new(command["sql"].as_str().ok_or(Error::Input)?, &id).map_err(Error::Shared)?;
    for (key, value) in command["settings"].as_object().ok_or(Error::Input)? {
        query = query
            .setting(
                key,
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            )
            .map_err(Error::Shared)?;
    }
    for (key, value) in super::worker_parameters::scope_parameters(command)? {
        query = query.parameter(&key, value).map_err(Error::Shared)?;
    }
    query = query
        .setting("output_format_arrow_string_as_string", "1")
        .map_err(Error::Shared)?;
    let mut execution = client.fetch_arrow(query).await.map_err(Error::Shared)?;
    let mut state = ArrowDigest::new(count, expected);
    let mut consumer = Duration::ZERO;
    let mut peak_batch = 0;
    let mut first_batch = None;
    let mut all_rows = None;
    loop {
        let next = if optimistic {
            execution.next_provisional_batch().await
        } else {
            execution.next_batch().await
        };
        let batch = match next {
            Ok(Some(batch)) => batch,
            Ok(None) => break,
            Err(error) => {
                let Some((kind, code)) = native_refusal(error, execution.outcome()) else {
                    return Err(Error::Shared(error));
                };
                let elapsed = start.elapsed();
                let m = execution.metrics();
                let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
                // Discard any partial digest/values. Failed work and its proof
                // remain in the cost, but never enter successful latency.
                return Ok(json!({
                    "refusal_protocol":"budget-refusal-v1",
                    "completion":"BudgetRefused","result_verified":false,
                    "query_id":id,"budget_kind":kind,"native_error_code":code,
                    "native_terminal_outcome":"Failed","failed_client_ms":ms(elapsed),
                    "failure_clock_boundary":"request-construction-through-budget-refusal-v1",
                    "client_path":client_path,
                    "consumer_wall_ms":ms(consumer),"queued_ms":ms(m.queued),
                    "identity_ms":ms(m.identity),"data_poll_ms":ms(m.data_poll),
                    "first_batch_ms":m.first_batch.map(ms),"stream_end_ms":m.stream_end.map(ms),
                    "terminal_ms":ms(m.terminal),"execution_total_ms":ms(m.total),
                    "control_requests":m.control_requests,"flush_attempts":m.flush_attempts,
                    "flush_failures":m.flush_failures,"flush_ms":ms(m.flush_time),
                    "observe_ms":ms(m.observe_time),"polls":m.polls,
                    "poll_sleeps":m.poll_sleeps,"poll_wait_ms":ms(m.poll_wait),
                    "decoded_bytes":execution.decoded_bytes_delivered(),
                    "peak_charged_batch_bytes":peak_batch,"terminal_policy":terminal_policy,
                    "http_body_wait_ms":m.transfer.map(ms),"decode_wall_ms":m.decode.map(ms),
                    "framing_ms":ms(m.framing),"http_body_bytes":m.wire_bytes
                }));
            }
        };
        if optimistic && first_batch.is_none() {
            first_batch = Some(start.elapsed());
        }
        peak_batch = peak_batch.max(batch.charged_bytes());
        let consume_start = Instant::now();
        state.push(&batch)?;
        consumer += consume_start.elapsed();
        // This is availability of the expected rows after consumption, not
        // completeness: later batches or terminal proof can still invalidate it.
        if optimistic && all_rows.is_none() && state.rows > 0 && expected_rows == Some(state.rows) {
            all_rows = Some(start.elapsed());
        }
        // The charged batch remains alive during hashing, then releases before
        // the next poll. No untracked arrays or provisional samples escape.
    }
    let stream_eof = optimistic.then(|| start.elapsed());
    schema(execution.schema().ok_or(Error::Values)?.as_ref(), count)?;
    let mut confirmation_started = None;
    let mut confirmation = None;
    let mut confirmed = None;
    if optimistic {
        let confirmation_start = Instant::now();
        confirmation_started = Some(confirmation_start.duration_since(start));
        execution
            .confirm_completion()
            .await
            .map_err(Error::Shared)?;
        confirmation = Some(confirmation_start.elapsed());
        confirmed = Some(start.elapsed());
    }
    let consume_start = Instant::now();
    let (rows, digest) = state.finish(execution.outcome())?;
    let total_ms = finish_verified(command, rows, &digest, || {
        consumer += consume_start.elapsed();
        start.elapsed()
    })?
    .as_secs_f64()
        * 1000.0;
    let m = execution.metrics();
    let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
    let mut report = json!({"query_id":id,"client_ms":total_ms,"rows":rows,"result_digest":digest,
        "clock_boundary":CLOCK_BOUNDARY,"result_verified":true,
        "completion":"Succeeded","client_path":client_path,
        "timing_contract":"terminal-inclusive; admission, identity, data polling, consumption and successful authenticated terminal proof",
        "headers_ms":null,"request_body_ms":null,"decode_wall_ms":m.decode.map(ms),"bytes":null,"elastic_took_ms":null,
        "http_body_wait_ms":m.transfer.map(ms),"framing_ms":ms(m.framing),"http_body_bytes":m.wire_bytes,
        "consumer_wall_ms":ms(consumer),"queued_ms":ms(m.queued),"identity_ms":ms(m.identity),
        "data_poll_ms":ms(m.data_poll),"first_batch_ms":m.first_batch.map(ms),"stream_end_ms":m.stream_end.map(ms),
        "terminal_ms":ms(m.terminal),"execution_total_ms":ms(m.total),"control_requests":m.control_requests,
        "flush_attempts":m.flush_attempts,"flush_failures":m.flush_failures,"flush_ms":ms(m.flush_time),
        "observe_ms":ms(m.observe_time),"polls":m.polls,"poll_sleeps":m.poll_sleeps,"poll_wait_ms":ms(m.poll_wait),
        "decoded_bytes":execution.decoded_bytes_delivered(),"peak_charged_batch_bytes":peak_batch,
        "resource_note":"charged Arrow allocations are not RSS; HTTP entity bytes exclude headers/TLS; body wait includes server/scheduling; phase clocks overlap",
        "terminal_policy":terminal_policy});
    if optimistic {
        report["timing_contract"] = json!(
            "terminal-inclusive client_ms; provisional availability is separate and cannot establish successful completion; all samples require authenticated terminal proof and final digest verification"
        );
        report["optimistic_result_status"] = json!("ProvisionalUntilConfirmation");
        report["optimistic_first_batch_ms"] = json!(first_batch.map(ms));
        report["optimistic_all_rows_ms"] = json!(all_rows.map(ms));
        report["optimistic_stream_eof_ms"] = json!(stream_eof.map(ms));
        report["confirmation_started_ms"] = json!(confirmation_started.map(ms));
        report["confirmation_ms"] = json!(confirmation.map(ms));
        report["confirmed_ms"] = json!(confirmed.map(ms));
        report["optimistic_clock_boundary"] = json!(
            "offsets from request construction; first batch before consumption; all expected nonzero rows after consumption (null for zero rows); stream EOF; terminal proof starts/finishes; client_ms includes final oracle verification"
        );
        report["component_timing_note"] = json!(
            "first_batch_ms and stream_end_ms use Antalya execution start; optimistic offsets use worker request start; body wait includes server/scheduling; independent Arrow/framing work is exposed; terminal, flush, observe and poll-wait clocks overlap and must not be summed"
        );
    }
    Ok(report)
}

pub(super) fn policy(command: &Value) -> Result<&'static str> {
    match command.get("terminal_policy") {
        None => Ok("flush_first"),
        Some(Value::String(s)) if s == "flush_first" => Ok("flush_first"),
        Some(Value::String(s)) if s == "in_band" => Ok("in_band"),
        _ => Err(Error::Input),
    }
}

#[cfg(test)]
#[path = "shared_engine_optimistic_tests.rs"]
mod optimistic_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, StringArray, UInt64Array};
    use arrow_schema::{DataType, Field, Schema};
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn native_budget_response_requires_failed_private_proof_and_data_phase() {
        for (code, kind) in [(159, "time"), (241, "memory")] {
            let error = antalya_rs::Error::at(Phase::Data, Cause::ServerResponse(Some(code)));
            assert_eq!(native_refusal(error, Outcome::Failed), Some((kind, code)));
            for outcome in [
                Outcome::Running,
                Outcome::Unknown,
                Outcome::Cancelled,
                Outcome::Succeeded,
            ] {
                assert!(native_refusal(error, outcome).is_none());
            }
            for phase in [
                Phase::Queue,
                Phase::Identity,
                Phase::Control,
                Phase::Progress,
                Phase::Cancellation,
            ] {
                assert!(
                    native_refusal(
                        antalya_rs::Error::at(phase, Cause::ServerResponse(Some(code))),
                        Outcome::Failed
                    )
                    .is_none()
                );
            }
        }
        for cause in [
            Cause::Timeout,
            Cause::InvalidProof,
            Cause::QueryFailed,
            Cause::ServerResponse(None),
            Cause::ServerResponse(Some(497)),
        ] {
            assert!(
                native_refusal(antalya_rs::Error::at(Phase::Data, cause), Outcome::Failed)
                    .is_none()
            );
        }
    }

    pub(super) fn batch(ids: Vec<Option<u64>>, values: Option<Vec<Option<&str>>>) -> RecordBatch {
        let mut fields = vec![Field::new("id", DataType::UInt64, true)];
        let mut columns: Vec<ArrayRef> = vec![Arc::new(UInt64Array::from(ids))];
        if let Some(values) = values {
            fields.push(Field::new("value", DataType::Utf8, true));
            columns.push(Arc::new(StringArray::from(values)));
        }
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
    }
    #[test]
    fn explain_only_accepts_one_bounded_nonnull_text_cell() {
        let text = |values: Vec<Option<&str>>| {
            RecordBatch::try_from_iter(vec![(
                "explain",
                Arc::new(StringArray::from(values)) as ArrayRef,
            )])
            .unwrap()
        };
        assert_eq!(
            explain_cell(&text(vec![Some("[]")])).unwrap(),
            Some("[]".into())
        );
        assert!(explain_cell(&text(vec![None])).is_err());
        assert!(explain_cell(&text(vec![Some("[]"), Some("[]")])).is_err());
        assert!(explain_cell(&batch(vec![Some(1)], None)).is_err());
        let large = "x".repeat(262145);
        assert!(explain_cell(&text(vec![Some(large.as_str())])).is_err());
    }

    #[test]
    fn complete_arrow_values_match_json_across_batches() {
        let mut d = ArrowDigest::new(false, 3);
        d.push(&batch(
            vec![Some(1), Some(2)],
            Some(vec![Some(""), Some("héllo\0\\")]),
        ))
        .unwrap();
        d.push(&batch(vec![Some(3)], Some(vec![Some("cmd.exe")])))
            .unwrap();
        let json = json!({"rows":3,"data":[{"id":1,"value":""},{"id":2,"value":"héllo\0\\"},{"id":3,"value":"cmd.exe"}]});
        assert_eq!(
            d.finish(Outcome::Succeeded).unwrap(),
            crate::checked_digest(&json, "antalya", "page", 3).unwrap()
        );
    }
    #[test]
    fn counts_and_empty_pages_require_verified_completion() {
        let mut count = ArrowDigest::new(true, 0);
        count.push(&batch(vec![Some(0)], None)).unwrap();
        assert_eq!(
            count.finish(Outcome::Succeeded).unwrap(),
            crate::checked_digest(&json!({"rows":1,"data":[{"id":0}]}), "antalya", "count", 0)
                .unwrap()
        );
        let empty = ArrowDigest::new(false, 0);
        assert_eq!(
            empty.finish(Outcome::Succeeded).unwrap(),
            crate::checked_digest(&json!({"rows":0,"data":[]}), "antalya", "page", 0).unwrap()
        );
        for outcome in [
            Outcome::Running,
            Outcome::Unknown,
            Outcome::Failed,
            Outcome::Cancelled,
        ] {
            assert!(ArrowDigest::new(false, 0).finish(outcome).is_err());
        }
        assert!(
            ArrowDigest::new(true, 0)
                .finish(Outcome::Succeeded)
                .is_err()
        );
    }
    #[test]
    fn duplicates_order_nulls_and_extra_columns_fail() {
        let mut d = ArrowDigest::new(false, 2);
        d.push(&batch(vec![Some(2)], Some(vec![Some("ok")])))
            .unwrap();
        for id in [1, 2] {
            assert!(
                d.push(&batch(vec![Some(id)], Some(vec![Some("bad")])))
                    .is_err()
            );
        }
        for b in [
            batch(vec![None], Some(vec![Some("x")])),
            batch(vec![Some(1)], Some(vec![None])),
            batch(vec![Some(1)], None),
        ] {
            assert!(ArrowDigest::new(false, 1).push(&b).is_err());
        }
        let wrong = RecordBatch::try_from_iter(vec![(
            "wrong",
            Arc::new(UInt64Array::from(vec![1])) as ArrayRef,
        )])
        .unwrap();
        assert!(ArrowDigest::new(true, 1).push(&wrong).is_err());
        assert!(
            ArrowDigest::new(true, 1)
                .push(&batch(vec![Some(2)], None))
                .is_err()
        );
    }
    #[test]
    fn oversized_values_and_row_totals_fail() {
        let text = "x".repeat(8193);
        assert!(
            ArrowDigest::new(false, 1)
                .push(&batch(vec![Some(1)], Some(vec![Some(&text)])))
                .is_err()
        );
        let many = batch((0..32769).map(Some).collect(), Some(vec![Some("x"); 32769]));
        assert!(ArrowDigest::new(false, 32769).push(&many).is_err());
    }
}
