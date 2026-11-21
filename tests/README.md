<!-- SPDX-License-Identifier: Apache-2.0 -->
# Consumer tests

- [FDW](fdw/README.md): PostgreSQL integration, planner checks and a separate
  Rust transport/decoder harness.
- [Transport](transport/README.md): deterministic HTTP, Arrow and worker
  regression tests using the companion client.
- `operations/`: unit tests for the deployment diagnostic and migration-plan
  tool, plus a native check restricted to an owned Antalya fixture.

Library tests, semantic differential runners and the Iceberg/Puffin end-to-end
test live in [`libwildcard/tests`](../libwildcard/tests). Run them directly from
that canonical location. See [testing](../docs/testing.md) for commands and
prerequisites.
