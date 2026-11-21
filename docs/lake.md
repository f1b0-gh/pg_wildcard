<!-- SPDX-License-Identifier: Apache-2.0 -->
# Lake indexing

`wc-lake` builds conservative n-gram indexes from local Parquet inputs and stores
them as Iceberg Puffin sidecars. ClickHouse/Antalya still reads the data and
evaluates the exact predicate. The CLI does not crawl a catalog, download S3
objects or schedule indexing: callers supply files and their exact native
`_path` values.

Build the standalone binary with the library's `cold` feature; see
[build instructions](build.md). The compatibility workspace also exposes a
`wc-lake` target using the same source.

## File-level workflow

```bash
wc-lake index --column url --views raw,decoded,revdomain \
  --field-id 2 --snapshot-id 42 --sequence-number 3 \
  --out urls.puffin local.parquet=s3://bucket/table/data/file.parquet
wc-lake inspect urls.puffin
wc-lake prune urls.puffin --kind wildcard --view decoded --pattern '*/wp-admin/*'
wc-lake sql urls.puffin --table lake.http --select 'ts, url' \
  --kind wildcard --view decoded --pattern '*/wp-admin/*'
```

Use the actual snapshot, sequence and field IDs and the exact `_path` reported
by the engine. An input without `LOCAL=LAKE_PATH` uses the same path for both.
`index` defaults to view `raw` and `--max-value-bytes 8192`; omitted metadata IDs
are `-1`, so provide real IDs when binding to an Iceberg snapshot. Overlong
values make the whole file a candidate. NULL values do not match the strict
search predicates. `prune` prints candidates within the supplied inventory;
it does not enumerate unknown current files.

`sql` accepts `--where EXPR` and `--prune off`. `--select` and `--where` are
trusted SQL fragments, not data literals. The generated SQL retains the exact
matching predicate.

For PostgreSQL, export the index:

```bash
wc-lake export-pg urls.puffin --name http_url --out-dir /tmp/http-url-index
```

Then load it in psql, replacing the previous index atomically:

```sql
BEGIN;
DELETE FROM wc_lake_index WHERE index_name = 'http_url';
\copy wc_lake_index(index_name,column_name,views,snapshot_id) FROM '/tmp/http-url-index/index.csv' CSV
\copy wc_lake_file FROM '/tmp/http-url-index/files.csv' CSV
\copy wc_lake_gram FROM '/tmp/http-url-index/grams.csv' CSV
COMMIT;
```

The [SQL functions](sql-api.md) query these tables. A foreign table's `lake_index`
option enables file-level pruning automatically for supported predicates.

## Coverage and snapshot rules

The CLI `sql` command uses `lake_query_with_coverage`: it excludes only known
impossible immutable files. Its guard keeps files outside the indexed inventory,
even when no known file is a candidate. An unknown file must remain searchable;
an empty candidate list is not evidence that the current table is empty.

The PostgreSQL `wc_lake_sql` function and FDW `lake_index` option instead use
candidate allowlists. They do **not** add unknown-file fallback or validate the
remote snapshot automatically. Use them only when the loaded inventory covers
the queried snapshot. Pin the FDW's `iceberg_snapshot_id` setting where supported,
and verify that it matches the index; otherwise omit `lake_index` and search
unpruned, or use coverage-aware SQL externally. Reading a newer snapshot through
an old allowlist can miss matches. Local rechecks cannot repair that omission.

All reuse and exclusion depend on immutable paths. An overwritten object at the
same path invalidates its index. Snapshot metadata in an index is supplied by
the caller; it is not proof of catalog identity or completeness. Authenticate
sidecars and inventories and retain the original source's delete handling,
authorization restrictions and exact search predicate.

## Puffin contract

The file-level blob type is `wildcard-ngram-v1`. It contains a `WCNG` header,
version 1, view bitmask, file paths/counts/flags and delta-encoded gram postings.
View IDs remain `raw=0`, `lower=1`, `decoded=2`, `revdomain=3`, `winpath=4`.
The `has_unindexed` file flag preserves candidates whose values exceed the
indexing budget; a view absent from the index keeps all indexed files.

Metadata includes the column field ID, snapshot/sequence, `column`, `views`,
`files`, `grams` and `max-value-bytes`. Writers use zstd blobs and plain footers;
readers also support LZ4 footers and zstd/LZ4 blob compression. Decoders validate
bounds and posting/varint overflow. These are exact postings, but matching
trigrams still does not prove a full pattern match.

Keep these blobs in **separate Puffin files**, never mixed with deletion
vectors or sketches. ClickHouse's deletion-vector reader does not accept this
custom blob type. `stats-json FILE --statistics-path PATH` emits an optional
Iceberg `set-statistics` update; it does not publish the file or commit metadata.

## Experimental group and identity APIs

The following CLI commands are available for consumers implementing stronger
pruning. They are not enabled by the PostgreSQL FDW and are not automatically
integrated into an Iceberg catalog.

| Command | Output or behavior |
| --- | --- |
| `index-groups`, `plan-groups` | Group postings and per-file inclusive guard ranges |
| `refresh-groups` | Reuse immutable files; emit matching file/group sidecars |
| `index-identities`, `plan-identities` | Sparse UTF-8 identity candidates and optional SQL guards |
| `refresh-identities` | Reuse complete immutable-file identity indexes |
| `refresh-search` | Construct file, group and identity sidecars from one projected input pass |
| `segment-budget` | Check bounded aggregate Puffin envelopes; not identity or coverage certification |

Group bindings include table UUID, search/guard column names and field IDs.
`--guard-type int64` is the default and writes `wildcard-rowgroup-v1`;
`timestamp_us_utc` requires Arrow UTC microsecond timestamps and writes
`wildcard-rowgroup-v2`. Timestamp SQL guards support 1900–2299 UTC; out-of-range
groups widen. `plan-groups --range-sql on` adds validated expressions, not a
complete query.

Sparse identity construction requires a UTF-8 guard and writes
`wildcard-sparse-identity-v2`; readers also accept v1. `plan-identities
--guard-sql on` adds quoted identity predicates. Plans use `full_scan` and
per-file ranges/values: `null` means unrestricted, `[]` excludes only that known
file. Every unlisted file must still be read. A guard is a necessary condition,
not proof of identity uniqueness or a replacement for exact matching.

Refresh commands require the entire current inventory, matching bindings,
views and value budgets, and trusted immutable reuse inputs. They read new
files and remove omitted files; they do not hash unchanged remote objects.
`refresh-search` requires paired group/identity caches or neither. Directory
outputs use a fresh destination and write `manifest.json` last, after their
separate sidecars. Failed generations may leave partial unpublished files.
The marker does not provide power-loss durability, scheduling, concurrent
publisher coordination or automatic consumer reload.

Group/identity sidecars have bounded capacities, including 128 files, 1 million
postings and 16 MiB per file; groups additionally allow 4,096 groups. Exceeded
construction budgets and invalid bindings are errors or documented conservative
fallbacks, not permission to drop candidates. Consumer integration must pin all
query branches to the same snapshot and preserve unknown-file fallback. See
[testing](testing.md) for the available verification paths.
