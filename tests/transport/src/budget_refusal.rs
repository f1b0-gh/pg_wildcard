// SPDX-License-Identifier: Apache-2.0
//! Bounded, explicit refusal outcomes; never a verified empty result.
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Refusal {
    kind: &'static str,
    elastic_type: &'static str,
}

impl Refusal {
    pub(super) fn sample(&self, query_id: &str, elapsed: Duration) -> Value {
        json!({
            "refusal_protocol": "budget-refusal-v1",
            "completion": "BudgetRefused",
            "result_verified": false,
            "query_id": query_id,
            "budget_kind": self.kind,
            "elastic_refusal": self.elastic_type,
            "failed_client_ms": elapsed.as_secs_f64() * 1000.0,
            "failure_clock_boundary": "request-construction-through-budget-refusal-v1"
        })
    }
}

fn leaf(value: &Value) -> Option<Refusal> {
    let error = value.as_object()?;
    if ["caused_by", "root_cause", "failed_shards", "suppressed"]
        .iter()
        .any(|key| error.contains_key(*key))
    {
        return None;
    }
    let (kind, elastic_type) = match error.get("type")?.as_str()? {
        "circuit_breaking_exception" => ("memory", "circuit_breaking_exception"),
        "es_rejected_execution_exception" => ("admission", "es_rejected_execution_exception"),
        "search_timeout_exception" if error.get("timed_out") == Some(&Value::Bool(true)) => {
            ("time", "search_timeout_exception")
        }
        _ => return None,
    };
    Some(Refusal { kind, elastic_type })
}

fn wrapped(value: &Value) -> Option<Refusal> {
    let error = value.as_object()?;
    // Qualified against Elasticsearch 9.5.6 with partial results disabled.
    // Only these fixed shapes are accepted: no recursive cause search.
    if error.get("type")?.as_str()? != "search_phase_execution_exception"
        || error.get("phase")?.as_str()? != "query"
        || error.get("grouped")? != true
        || error.contains_key("suppressed")
    {
        return None;
    }
    let roots = error.get("root_cause")?.as_array()?;
    let shards = error.get("failed_shards")?.as_array()?;
    if !(1..=32).contains(&roots.len()) || !(1..=32).contains(&shards.len()) {
        return None;
    }
    let refusal = leaf(roots.first()?)?;
    if roots.iter().any(|root| leaf(root) != Some(refusal)) {
        return None;
    }
    for shard in shards {
        let shard = shard.as_object()?;
        shard.get("shard")?.as_u64()?;
        for key in ["index", "node"] {
            if shard.get(key)?.as_str()?.len() > 256 {
                return None;
            }
        }
        if leaf(shard.get("reason")?) != Some(refusal) {
            return None;
        }
    }
    if let Some(cause) = error.get("caused_by") {
        // The qualified admission response repeats the same rejection in a
        // two-node cause chain. Reject extra depth or any different cause.
        let cause = cause.as_object()?;
        if refusal.kind != "admission"
            || cause.get("type")?.as_str()? != "es_rejected_execution_exception"
            || ["root_cause", "failed_shards", "suppressed"]
                .iter()
                .any(|key| cause.contains_key(*key))
            || leaf(cause.get("caused_by")?) != Some(refusal)
        {
            return None;
        }
    }
    Some(refusal)
}

pub(super) fn elastic(status: u16, value: &Value) -> Option<Refusal> {
    if status == 429 && value["status"].as_u64() == Some(429) {
        return match value["error"]["type"].as_str()? {
            "circuit_breaking_exception" | "es_rejected_execution_exception" => {
                leaf(&value["error"])
            }
            "search_phase_execution_exception" => wrapped(&value["error"]),
            _ => None,
        };
    }
    if status == 200 && value["timed_out"] == true && value.get("error").is_none() {
        let shards = value["_shards"].as_object()?;
        let total = shards.get("total")?.as_u64()?;
        if total > 0
            && shards.get("successful")?.as_u64()? == total
            && shards.get("failed")?.as_u64()? == 0
            && value["hits"]["hits"].as_array()?.len() <= 32768
        {
            return Some(Refusal {
                kind: "time",
                elastic_type: "timed_out",
            });
        }
    }
    None
}
