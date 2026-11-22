// Copyright 2026 The pg_wildcard Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeSet;

/// Exclude only known impossible immutable files, preserving unknown files.
///
/// `indexed` is the exact inventory of a trusted index and `candidates` includes
/// every indexed file that might match. Paths must refer to immutable data.
/// An empty candidate list still retains every file outside the inventory.
/// Returns `None` (no path restriction) if no file can be excluded or if the
/// complete escaped filter would exceed 64 KiB. The exact predicate must still
/// be evaluated by the caller, including when this function returns `None`.
pub fn lake_path_filter(indexed: &[String], candidates: &[String]) -> Option<String> {
    const MAX_FILTER_BYTES: usize = 64 * 1024;
    let chosen: BTreeSet<_> = candidates.iter().collect();
    let excluded: BTreeSet<_> = indexed
        .iter()
        .filter(|path| !chosen.contains(path))
        .collect();
    if excluded.is_empty() {
        return None;
    }

    let mut filter = String::from("_path NOT IN (");
    for (i, path) in excluded.into_iter().enumerate() {
        let separator = if i == 0 { "" } else { ", " };
        let remaining = MAX_FILTER_BYTES.checked_sub(filter.len() + separator.len() + 1)?;
        // Check raw length before quoting so a huge individual path cannot
        // cause an unbounded intermediate SQL allocation.
        if path.len() > remaining.saturating_sub(2) {
            return None;
        }
        let quoted = super::quote(path);
        if quoted.len() > remaining {
            return None;
        }
        filter.push_str(separator);
        filter.push_str(&quoted);
    }
    filter.push(')');
    Some(filter)
}
