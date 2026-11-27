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

## Shared skills

Use [skills-dev](https://github.com/f1b0-gh/skills-dev) for relevant work:
`skills/rust-discipline/SKILL.md` for Rust policy, `skills/rust-skills/` for
selected Rust references, and `skills/clickhouse-sql/SKILL.md` for ClickHouse.
Read these in the shared repository or an installed copy; project contracts,
dependency constraints and validation commands take precedence over examples.
Keep maintained instructions and documentation in English.
