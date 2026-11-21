# Contributing

- Keep changes focused; update current English documentation and relevant tests.
- `libwildcard/` is the canonical, standalone implementation. Keep compatibility
  crates as re-exports and do not add parent or client dependencies to the library.
- Match ClickHouse semantics. Pruning must never lose a match; preserve raw data,
  exact rechecks, coverage bindings and conservative fallbacks.
- Treat inputs as hostile: bound work and decoding, use linear-time regex, and
  escape generated SQL. Keep wildcard Puffin indexes separate from other blobs.
- Preserve Apache-2.0 compatibility and third-party notices; review new licenses.
- Follow [build](docs/build.md) and [test](docs/testing.md) instructions. Report
  checks that could not run. Keep reports, experiments and local work in `.local/`.
