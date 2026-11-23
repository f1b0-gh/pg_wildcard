// SPDX-License-Identifier: Apache-2.0
//! Runtime-only adaptive plan selection; no PostgreSQL callbacks or cached results.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
const CACHE_ENTRIES: usize = 32;
const KEY_BYTES: usize = 65536;
const TTL: Duration = Duration::from_secs(60);
struct Entry {
    exact: String,
    candidate: String,
    settings: Option<String>,
    selected: bool,
    reason: &'static str,
    created: Instant,
}
#[derive(Default)]
pub(super) struct PlanCache {
    entries: VecDeque<Entry>,
}
impl PlanCache {
    fn key_fits(exact: &str, candidate: &str, settings: Option<&str>) -> bool {
        exact
            .len()
            .saturating_add(candidate.len())
            .saturating_add(settings.map_or(0, str::len))
            <= KEY_BYTES
    }
    fn get(
        &mut self,
        exact: &str,
        candidate: &str,
        settings: Option<&str>,
        now: Instant,
    ) -> Option<(bool, &'static str)> {
        self.entries
            .retain(|e| now.saturating_duration_since(e.created) < TTL);
        let position = self.entries.iter().position(|e| {
            e.exact == exact && e.candidate == candidate && e.settings.as_deref() == settings
        })?;
        let entry = self.entries.remove(position)?;
        let result = (entry.selected, entry.reason);
        // Promote reuse without extending the insertion TTL: this is only a
        // plan preference, and fresh estimates must still be sampled regularly.
        self.entries.push_back(entry);
        Some(result)
    }
    fn put(
        &mut self,
        exact: &str,
        candidate: &str,
        settings: Option<&str>,
        selected: bool,
        reason: &'static str,
        now: Instant,
    ) {
        if !Self::key_fits(exact, candidate, settings) {
            return;
        }
        self.entries.retain(|e| {
            !(e.exact == exact && e.candidate == candidate && e.settings.as_deref() == settings)
        });
        if self.entries.len() >= CACHE_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            exact: exact.into(),
            candidate: candidate.into(),
            settings: settings.map(str::to_owned),
            selected,
            reason,
            created: now,
        });
    }
}
use antalya_rs::{AntalyaClient, Error, ExecutionMetrics};
use std::sync::{Arc, Mutex};
pub(super) type SharedCache = Arc<Mutex<PlanCache>>;
#[derive(Clone, Default)]
pub struct Preparation {
    pub elapsed: Duration,
    pub probe_queries: u64,
    pub cache_hits: u64,
    pub selected: bool,
    pub reason: &'static str,
    pub transfer: Duration,
    pub decode: Duration,
    pub framing: Duration,
    pub terminal: Duration,
    pub body_bytes: u64,
    pub controls: u64,
}
impl Preparation {
    pub fn accumulate(&mut self, next: &Self) {
        self.elapsed = self.elapsed.saturating_add(next.elapsed);
        self.probe_queries = self.probe_queries.saturating_add(next.probe_queries);
        self.cache_hits = self.cache_hits.saturating_add(next.cache_hits);
        self.transfer = self.transfer.saturating_add(next.transfer);
        self.decode = self.decode.saturating_add(next.decode);
        self.framing = self.framing.saturating_add(next.framing);
        self.terminal = self.terminal.saturating_add(next.terminal);
        self.body_bytes = self.body_bytes.saturating_add(next.body_bytes);
        self.controls = self.controls.saturating_add(next.controls);
        self.selected = next.selected;
        self.reason = next.reason;
    }
    fn execution(&mut self, m: ExecutionMetrics) {
        self.transfer = self.transfer.saturating_add(m.transfer.unwrap_or_default());
        self.decode = self.decode.saturating_add(m.decode.unwrap_or_default());
        self.framing = self.framing.saturating_add(m.framing);
        self.terminal = self.terminal.saturating_add(m.terminal);
        self.body_bytes = self.body_bytes.saturating_add(m.wire_bytes);
        self.controls = self.controls.saturating_add(m.control_requests);
    }
}

pub(super) async fn select(
    client: &AntalyaClient,
    cache: &SharedCache,
    exact: &str,
    candidate: &str,
    settings: Option<&str>,
) -> Result<Preparation, Error> {
    let started = Instant::now();
    let mut p = Preparation::default();
    if !PlanCache::key_fits(exact, candidate, settings) {
        p.reason = "sql_budget";
        p.elapsed = started.elapsed();
        return Ok(p);
    }
    let cached = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(exact, candidate, settings, started);
    if let Some((selected, reason)) = cached {
        p.selected = selected;
        p.reason = reason;
        p.cache_hits = 1;
        p.elapsed = started.elapsed();
        return Ok(p);
    }
    let base = estimate(client, exact, settings, &mut p).await?;
    let alternate = if base.is_some() {
        estimate(client, candidate, settings, &mut p).await?
    } else {
        None
    };
    let decision = libwildcard::antalya::choose_candidate(base, alternate);
    p.selected = decision.use_candidate();
    p.reason = decision.reason();
    cache.lock().unwrap_or_else(|e| e.into_inner()).put(
        exact,
        candidate,
        settings,
        p.selected,
        p.reason,
        Instant::now(),
    );
    p.elapsed = started.elapsed();
    Ok(p)
}
async fn estimate(
    client: &AntalyaClient,
    sql: &str,
    settings: Option<&str>,
    p: &mut Preparation,
) -> Result<Option<libwildcard::antalya::PlanEstimate>, Error> {
    // SQL is an internally generated alternative, not an analyst-provided EXPLAIN.
    let query = super::make_query(&format!("EXPLAIN json=1, indexes=1 {sql}"), settings)
        .map_err(|_| Error::InvalidRequest)?
        .setting("max_result_rows", "16")?
        .setting("max_result_bytes", "1048576")?;
    p.probe_queries += 1;
    let mut execution = client.fetch_arrow(query).await?;
    let mut json = None;
    let mut valid = true;
    let mut budget = ProbeBudget::default();
    while let Some(batch) = execution.next_batch().await? {
        budget.add(batch.num_rows() as u64, batch.charged_bytes() as u64)?;
        if batch.num_rows() == 0 {
            continue;
        }
        if batch.num_columns() != 1 || batch.num_rows() != 1 || json.is_some() {
            valid = false;
            continue;
        }
        match super::super::values::input(
            batch.column(0).as_ref(),
            0,
            super::super::values::Target::Text,
        ) {
            Ok(Some(super::super::values::Value::Text(text))) if text.len() <= 262144 => {
                // The estimate is parsed after the stream finishes, so retain
                // a bounded owned copy before this Arrow batch is released.
                json = Some(text.to_owned());
            }
            _ => valid = false,
        }
    }
    p.execution(execution.metrics());
    Ok(if valid {
        json.as_deref()
            .and_then(libwildcard::antalya::native_plan_estimate)
    } else {
        None
    })
}

#[derive(Default)]
struct ProbeBudget {
    rows: u64,
    batches: u64,
    bytes: u64,
}
impl ProbeBudget {
    fn add(&mut self, rows: u64, bytes: u64) -> Result<(), Error> {
        self.rows = self.rows.saturating_add(rows);
        self.batches = self.batches.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        if self.rows > 16 || self.batches > 64 || self.bytes > 1048576 {
            return Err(Error::LimitExceeded);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_budget_is_enforced_locally_even_if_server_ignores_settings() {
        let mut budget = ProbeBudget::default();
        assert!(budget.add(16, 1048576).is_ok());
        assert_eq!(budget.add(0, 1), Err(Error::LimitExceeded));
        assert_eq!(ProbeBudget::default().add(17, 1), Err(Error::LimitExceeded));
        let mut budget = ProbeBudget::default();
        for _ in 0..64 {
            assert!(budget.add(0, 0).is_ok());
        }
        assert_eq!(budget.add(0, 0), Err(Error::LimitExceeded));
    }
    #[test]
    fn cache_binds_both_queries_settings_and_insertion_ttl() {
        let now = Instant::now();
        let mut cache = PlanCache::default();
        cache.put(
            "exact",
            "candidate",
            Some("max_threads=1"),
            true,
            "candidate",
            now,
        );
        assert_eq!(
            cache
                .get("exact", "candidate", Some("max_threads=1"), now)
                .map(|x| x.0),
            Some(true)
        );
        assert!(
            cache
                .get("exact2", "candidate", Some("max_threads=1"), now)
                .is_none()
        );
        assert!(
            cache
                .get("exact", "candidate", Some("max_threads=2"), now)
                .is_none()
        );
        assert!(
            cache
                .get(
                    "exact",
                    "candidate",
                    Some("max_threads=1"),
                    now + Duration::from_secs(61)
                )
                .is_none()
        );
    }
    #[test]
    fn cache_hits_keep_hot_preferences_during_cold_key_churn() {
        let now = Instant::now();
        let mut cache = PlanCache::default();
        cache.put("hot", "candidate", None, true, "candidate", now);
        for i in 0..CACHE_ENTRIES - 1 {
            cache.put(
                &format!("cold{i}"),
                "candidate",
                None,
                false,
                "unknown",
                now,
            );
        }
        assert_eq!(
            cache.get("hot", "candidate", None, now),
            Some((true, "candidate"))
        );
        cache.put("new", "candidate", None, false, "unknown", now);
        assert_eq!(cache.entries.len(), CACHE_ENTRIES);
        assert_eq!(
            cache.get("hot", "candidate", None, now),
            Some((true, "candidate"))
        );
        assert!(cache.get("cold0", "candidate", None, now).is_none());
    }

    #[test]
    fn cache_recency_does_not_extend_insertion_ttl() {
        let now = Instant::now();
        let mut cache = PlanCache::default();
        cache.put("hot", "candidate", None, true, "candidate", now);
        for seconds in [30, 59] {
            assert!(
                cache
                    .get("hot", "candidate", None, now + Duration::from_secs(seconds))
                    .is_some()
            );
        }
        assert!(cache.get("hot", "candidate", None, now + TTL).is_none());
    }

    #[test]
    fn cache_is_bounded_and_large_keys_are_not_retained() {
        let mut cache = PlanCache::default();
        let now = Instant::now();
        for i in 0..40 {
            cache.put(&format!("e{i}"), "c", None, false, "unknown", now);
        }
        assert_eq!(cache.entries.len(), 32);
        cache.put(&"x".repeat(65537), "c", None, true, "candidate", now);
        assert_eq!(cache.entries.len(), 32);
    }
}
