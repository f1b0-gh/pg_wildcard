// Copyright 2026 The pg_wildcard Authors
// SPDX-License-Identifier: Apache-2.0
//! Shared matching and hot/cold search planning, independent of PostgreSQL.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

#[cfg(feature = "hot")]
pub mod antalya;
pub mod core;
#[cfg(feature = "cold")]
pub mod lake;
#[cfg(feature = "puffin")]
pub mod puffin;
