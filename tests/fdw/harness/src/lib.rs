// SPDX-License-Identifier: Apache-2.0
// Exercise the production transport and decoder without a PostgreSQL server.
#![allow(dead_code)]
#[path = "../../../../pg_wildcard/src/fdw/transport.rs"]
mod transport;
#[path = "../../../../pg_wildcard/src/fdw/values.rs"]
mod values;
