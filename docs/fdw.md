<!-- SPDX-License-Identifier: Apache-2.0 -->
# wildcard_fdw

`wildcard_fdw` is a scan-only PostgreSQL foreign data wrapper for
ClickHouse/Antalya search. It pushes exact search predicates, projects required
columns and optionally prunes indexed lake files. Joins and aggregates remain
local. It can coexist with pg_clickhouse for general analytics.

```sql
CREATE SERVER ch FOREIGN DATA WRAPPER wildcard_fdw
  OPTIONS (url 'https://antalya.example', database 'logs');
CREATE USER MAPPING FOR CURRENT_USER SERVER ch
  OPTIONS (bearer_token '<application-issued-token>');
CREATE FOREIGN TABLE http (
  id bigint, ts timestamptz, url text
) SERVER ch OPTIONS (table 'logs.http');

SELECT id, ts, url FROM http
WHERE wc_match(url, '*/wp-admin/*.php', 'decoded')
ORDER BY id LIMIT 20;
```

The source must expose compatible scalar types; use a typed UTC timestamp such
as `DateTime64(6, 'UTC')`. The application obtains and refreshes bearer tokens.
Legacy `user`/`password` mappings and URL credentials are refused. Token rotation
uses `ALTER USER MAPPING ... OPTIONS (SET bearer_token '...')`; cached client
identity includes the token. Protect catalog access to mapping credentials.

## Options

| Scope | Option | Default / meaning |
| --- | --- | --- |
| Server | `url` | `http://127.0.0.1:8123` |
| Server | `database` | `default` |
| Server | `timeout_ms` | `60000`; integer 1–300000 |
| Server | `terminal_policy` | `flush_first`; also `observe_first`, `observe_only`, `in_band` |
| Server or table | `settings` | Allowlisted comma-separated `key=value` pairs; table value overrides server value |
| Mapping | `bearer_token` | Required, nonempty |
| Table | `table` | Remote source |
| Table | `materialized_views` | Off; `true` uses conventional `<col>_<view>` columns |
| Table | `lake_index` | Optional name in `wc_lake_index` |
| Table | `rows` | Optional planner row estimate |
| Table | `candidate_columns` | Off; bounded comma-separated plain identifiers |
| Table | `candidate_prewhere` | Off; place candidate filter in PREWHERE when enabled |
| Table | `candidate_mode` | `manual`; `auto` enables bounded native plan comparison |

Authentication and endpoint options cannot be overridden on foreign tables.
Allowed settings are `max_threads` (1–64), `max_block_size` (1–16384; default
1024), `max_execution_time` (>0 and ≤300 seconds), `object_storage_cluster`
(a short cluster name), and `iceberg_snapshot_id` (positive integer). They are
encoded as HTTP settings, not interpolated as arbitrary SQL.

## Pushdown and rechecks

Constant-pattern `wc_match`, `wc_like` and `wc_regex` calls compile to exact
native predicates. Supported ordinary comparisons, LIKE/NOT LIKE, NULL tests
and Boolean combinations can also be pushed down. Every qual remains a local
PostgreSQL check. Correct pushdown returns the same rows: tests require zero
rows removed by local rechecks. Rechecks cannot recover remotely excluded rows.

Ordinary text equality/inequality and LIKE require a deterministic PostgreSQL
collation; ordering comparisons additionally require byte ordering. Unsupported
collations keep the predicates local and exclude them from candidate/lake
extraction. Extension `wc_*` matching follows its own [semantics](semantics.md),
independent of the column collation. The planner support hook preserves these
extension predicates on this wrapper.

Remote ordered limits are conservative: a simple single-source SELECT, integer
sort columns, constant nonnegative LIMIT/OFFSET and exact supported quals are
required. Joins, aggregates, windows, subqueries, DISTINCT, CTEs, row-security
quals and other unsupported shapes disable the optimization. The remote query
fetches `LIMIT + OFFSET` with explicit null ordering, up to one million rows;
PostgreSQL retains its own Sort/Limit and applies OFFSET once. Text, float and
timestamp ordering do not use this optimization.

`lake_index` intersects supported file candidates on the indexed column.
It does not validate coverage against the remote snapshot or retain unknown
new files automatically. Follow the [lake coverage rules](lake.md#coverage-and-snapshot-rules)
before enabling it.

## Arrow transport and completion

Execution uses the shared `antalya-rs::AntalyaClient` and ClickHouse Arrow
transport. Each backend runtime has one asynchronous worker and at most four
blocking workers, alongside a four-client identity cache and at most four active
scan producers. Arrow batches pass through a channel of capacity one.
PostgreSQL checks interrupts and converts values on its backend thread; workers
do not call PostgreSQL APIs. Query dispatch starts at first tuple demand, so
LIMIT 0 and unused branches do not execute remote queries. Rescans start a new
query. Plain EXPLAIN performs no network I/O.

The blocking-pool limit also covers system DNS work submitted through Tokio.
Aborting a lookup does not interrupt an already-running system resolver call:
that work can outlive the scan deadline and occupies its blocking worker until
it exits. The pool limit bounds running blocking workers, not queued work, DNS
duration, runtime shutdown time or all threads in the PostgreSQL process.

HTTP EOF alone is not successful completion. The default terminal policy
requires a fresh query-log observation bound to the query and a matching result
row count. The mapping principal needs access to the source and
`system.query_log`, with query/settings logging enabled. `flush_first` tries to
flush logs; `observe_first` first tries observation; `observe_only` avoids the
flush. Proof must arrive within the operation deadline. Missing evidence or
privileges fail the statement.

`in_band` requires compatible native `JSONEachPacketBase64` framing around
ArrowStream. It requires terminal progress, compatible result counts, complete
Arrow and clean HTTP EOF; late exception packets fail. Unsupported servers
fail rather than silently reducing the proof requirement. Normal LIMIT/cursor
close and rescans drain the stream and require terminal confirmation. Explicit
cancellation and error cleanup abort the scan.

Rows received before terminal success are provisional: a later transport,
engine, timeout or completion failure fails the SQL statement. Output already
streamed to a client cannot be retracted.

Terminal status has its own channel and never waits for space in the Arrow
queue. A timeout or an unconfirmed producer exit discards unread provisional
events before releasing scan admission, even if PostgreSQL is not polling.
Confirmed success and other errors preserve queued data before delivering their
terminal status; such queued batches can outlive the producer permit.
Sending to a closed queue is an unconfirmed failure, not successful completion.

Explicit cancellation clears the unread queue, aborts the local task and waits
up to two seconds for task destruction while checking PostgreSQL interrupts.
If destruction is delayed, cleanup reports that cancellation is still pending
and the task keeps its scan permit until it is destroyed. This acknowledges
local cleanup only; it does not prove that the remote query or a running DNS
lookup has stopped. Deadline cleanup also depends on the runtime being able to
poll and destroy the asynchronous task.

## Tuple boundary and budgets

The PostgreSQL database must use UTF-8. Initial Arrow schema validation also
applies to empty results; schemas must stay identical between batches. Only
required columns are decoded.

| PostgreSQL type | Accepted Arrow representation |
| --- | --- |
| `text`, unbounded `varchar` | UTF8/Binary, large variants, FixedSizeBinary; valid UTF-8 without NUL |
| `bytea` | Same families, preserving arbitrary bytes |
| `smallint`, `integer`, `bigint` | Signed/unsigned integer arrays within target range |
| `boolean` | Boolean or UInt8 restricted to 0/1 |
| `real` | Float32 |
| `double precision` | Float32 or Float64 |
| `date` | Date32 within the supported AD chrono range |
| `timestamp`, `timestamptz` | Timestamps in seconds/milliseconds/microseconds/nanoseconds, absent or UTC timezone, exactly representable in microseconds and within the supported AD chrono range |

NULL remains NULL. Domains, arrays, decimal/numeric, unsupported modifiers,
non-UTC timezone metadata, invalid text and overflow fail explicitly. Nanoseconds
must be divisible by 1000. Untyped integers are not accepted as timestamps;
expose a typed UTC projection if a server exports legacy DateTime as UInt32.
`timestamp` uses UTC wall-clock fields; `timestamptz` preserves the UTC instant.

Limits are one million result rows, 64 MiB server result bytes with overflow
errors, 16 MiB per decoded batch and 1 MiB per string field. `max_block_size`
is a requested block size, not a hard Arrow row cap or an increase to these
budgets. The batch byte check occurs after upstream Arrow decoding, so it is
not a preallocation defense against a malicious server. Configure trusted
destinations. An active scan can hold a current, queued and pending-send batch.

These limits cover different owners. The four-producer limit does not count
completed streams retaining confirmed queued batches, and the client identity
cache does not revoke clients still held by sessions. Client buffer leases,
PostgreSQL tuple copies, native decoding and allocator overhead are separate
from the per-batch check; the listed limits do not form a backend-wide RSS cap.

## Optional candidate staging and diagnostics

Candidate columns enable additional escaped scalar LIKE tests from conservative
grams. The exact predicate remains in place; unsupported branches widen.
PREWHERE requires native engine support and is never automatically enabled for
Iceberg. Candidate staging can add work and remains opt-in.

`candidate_mode='auto'` compares exact and candidate variants using bounded,
confirmed native EXPLAIN probes at first tuple demand. It recognizes a limited
single-MergeTree-reader plan shape and caches preferences by client identity.
Unknown plans use exact SQL; authentication, transport and confirmation errors
still fail. Preparation, execution and completion share the deadline.

`EXPLAIN ANALYZE` reports generated SQL and scan/transport counters. Row conversion
timing is collected only when executor timing is requested. With `TIMING OFF`,
zero conversion time means unmeasured. Body-wait time includes server execution
and scheduling; it is not pure network latency. Diagnostic clocks overlap and
must not be added as independent components of ordinary query latency.

See [testing](testing.md) for native and chDB checks and [operations](operations.md)
for deployment requirements.
