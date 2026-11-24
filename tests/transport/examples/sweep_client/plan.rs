// SPDX-License-Identifier: Apache-2.0
use super::Error;
use serde::Deserialize;

pub const MAX_PAIRS: usize = 100_000;
pub const MAX_SQL: usize = 200_000;
pub const TABLE: &str = "wc_staged";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub candidate_select: String,
    pub queries: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub database: String,
    pub queries: Vec<String>,
    pub workers: usize,
    pub source_rows: u64,
    pub staged: Option<Stage>,
    #[serde(default = "default_deadline")]
    pub deadline_ms: u64,
}
fn default_deadline() -> u64 {
    60_000
}
impl Plan {
    pub fn validate(&self) -> Result<(), Error> {
        let suffix = self
            .database
            .strip_prefix("wc_benchmark_")
            .ok_or(Error::Invalid("owned database required"))?;
        if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid("owned database identifier"));
        }
        if !matches!(self.workers, 1 | 4) || !(1..=60_000).contains(&self.deadline_ms) {
            return Err(Error::Invalid("workers/deadline"));
        }
        let bounded = |queries: &[String]| {
            !queries.is_empty()
                && queries.len() <= 8
                && queries
                    .iter()
                    .all(|q| !q.trim().is_empty() && q.len() <= MAX_SQL)
        };
        if !bounded(&self.queries) {
            return Err(Error::Invalid("query count/bytes"));
        }
        if let Some(stage) = &self.staged
            && (!bounded(&stage.queries)
                || stage.candidate_select.trim().is_empty()
                || stage.candidate_select.len() > MAX_SQL)
        {
            return Err(Error::Invalid("staging query bounds"));
        }
        Ok(())
    }
}
pub fn allows(source: u64, rows: u64, bytes: u64) -> bool {
    rows <= source
        && rows <= 100_000
        && bytes <= 64 << 20
        && (rows != 0 || bytes == 0)
        && u128::from(rows) * 1000 <= u128::from(source) * 100
}
#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> Plan {
        Plan {
            database: format!("wc_benchmark_{}", "a".repeat(32)),
            queries: vec!["SELECT 1".into()],
            workers: 4,
            source_rows: 1000,
            staged: None,
            deadline_ms: 60000,
        }
    }
    #[test]
    fn rejects_foreign_database_and_unbounded_commands() {
        let mut p = plan();
        assert!(p.validate().is_ok());
        p.database = "default".into();
        assert!(p.validate().is_err());
        p = plan();
        p.workers = 5;
        assert!(p.validate().is_err());
        p = plan();
        p.queries = vec!["SELECT 1".into(); 9];
        assert!(p.validate().is_err());
        p = plan();
        p.queries[0] = "x".repeat(MAX_SQL + 1);
        assert!(p.validate().is_err());
        p = plan();
        p.deadline_ms = 0;
        assert!(p.validate().is_err());
        p = plan();
        p.deadline_ms = 60001;
        assert!(p.validate().is_err());
    }
    #[test]
    fn admission_is_overflow_safe_and_never_truncates() {
        assert!(allows(1000, 100, 64 << 20));
        assert!(!allows(1000, 101, 1));
        assert!(!allows(1000, 0, 1));
        assert!(!allows(1000, 100, (64 << 20) + 1));
        assert!(allows(0, 0, 0));
        assert!(allows(u64::MAX, 100_000, 64 << 20));
        assert!(!allows(u64::MAX, 100_001, 0));
    }
}
