// SPDX-License-Identifier: Apache-2.0
//! Session-independent constants and PostgreSQL LIKE normalization.
use chrono::Datelike;

pub fn like(pattern: &str) -> Option<String> {
    // PostgreSQL rejects a dangling escape; do not hide its local error by
    // sending a remote predicate which could discard all input first.
    if pattern.bytes().rev().take_while(|&b| b == b'\\').count() % 2 != 0 {
        return None;
    }
    let pieces = libwildcard::core::pattern::parse_like_checked(pattern.as_bytes()).ok()?;
    Some(libwildcard::antalya::like_pattern(&pieces))
}

/// Convert the PostgreSQL 2000 epoch to exact Unix microseconds. Only send
/// constants in ClickHouse's documented DateTime64/Date32 calendar range;
/// infinities, BC dates and out-of-range values remain PostgreSQL predicates.
pub fn timestamp(pg_micros: i64) -> Option<i64> {
    let micros = pg_micros.checked_add(946_684_800_000_000)?;
    let date = chrono::DateTime::from_timestamp_micros(micros)?;
    (1900..=2299).contains(&date.year()).then_some(micros)
}

pub fn date(pg_days: i32) -> Option<String> {
    let epoch = chrono::NaiveDate::from_ymd_opt(2000, 1, 1)?;
    let date = epoch.checked_add_signed(chrono::Duration::days(i64::from(pg_days)))?;
    (1900..=2299)
        .contains(&date.year())
        .then(|| date.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_like_escapes_are_canonical_and_bounded() {
        assert_eq!(like(r"a\bc"), Some("abc".into()));
        assert_eq!(like(r"a\%b\_c\\d"), Some(r"a\%b\_c\\d".into()));
        assert_eq!(like(r"\é_%"), Some("é_%".into()));
        assert_eq!(like("abc\\"), None);
        assert_eq!(like(&"a".repeat(16 * 1024 + 1)), None);
    }

    #[test]
    fn temporal_epoch_conversion_preserves_microseconds() {
        assert_eq!(timestamp(0), Some(946_684_800_000_000));
        assert_eq!(timestamp(-946_684_800_000_001), Some(-1));
        assert_eq!(timestamp(123456), Some(946_684_800_123_456));
        assert_eq!(date(0), Some("2000-01-01".into()));
        assert_eq!(date(-1), Some("1999-12-31".into()));
        assert_eq!(timestamp(i64::MAX), None);
        assert_eq!(timestamp(i64::MIN), None);
        assert_eq!(date(i32::MAX), None);
        assert_eq!(date(i32::MIN), None);
        assert_eq!(date(365 * 600), None);
    }
}
