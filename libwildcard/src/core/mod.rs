// Copyright 2026 The pg_wildcard Authors
// SPDX-License-Identifier: Apache-2.0

//! `wildcard-core`: the semantics of pg_wildcard, independent of any engine.
//!
//! **ClickHouse is the engine of record.** Every function here is defined to
//! give the same answer as the ClickHouse expression `wildcard-ch` generates
//! for it (checked by differential tests against an embedded ClickHouse).
//! Postgres and the lake indexer use these definitions for local evaluation,
//! planning and pruning.
//!
//! Pipeline for one predicate:
//! 1. [`pattern`] / [`regex_plan`] turn a pattern into a [`query::GramQuery`]
//!    (trigrams every match must contain), used for pruning.
//! 2. [`verify::Matcher`] decides exactly, with ClickHouse semantics:
//!    - `LIKE` `_` and wildcard `?` match one UTF-8 **character**; invalid
//!      UTF-8 is never matched by `_`, `%`, `?`, `*` or `.`;
//!    - regexes use the RE2 dialect, `.` matches newline, `\d \w \s \b` are ASCII;
//!    - matching is linear time (no backtracking).
//!
//! **Soundness contract:** a gram plan never rejects a value the matcher
//! accepts. Property tests enforce it.

pub mod grams;
pub mod indicator;
pub mod pattern;
pub mod query;
pub mod re2;
pub mod regex_plan;
pub mod verify;
pub mod views;

pub use query::GramQuery;
pub use verify::{Kind, MatchCache, Matcher};
pub use views::View;
