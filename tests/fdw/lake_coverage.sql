-- SPDX-License-Identifier: Apache-2.0
-- psql -X -v ON_ERROR_STOP=1 -f tests/fdw/lake_coverage.sql
-- Run against a database with CREATE EXTENSION pg_wildcard already installed.
BEGIN;
INSERT INTO wc_lake_index(index_name, column_name, views, snapshot_id)
VALUES ('regression_coverage', 'url', 'raw', 1);
INSERT INTO wc_lake_file VALUES
    ('regression_coverage', 0, 'old', 1, false),
    ('regression_coverage', 1, 'unindexed', 1, true);
CREATE TEMP TABLE lake_current (_path text, url text);
INSERT INTO lake_current VALUES
    ('old', 'ordinary'), ('unindexed', 'evil.test'),
    ('new', 'evil.test'), ('new', 'ordinary');

CREATE SERVER coverage_source FOREIGN DATA WRAPPER wildcard_fdw OPTIONS (url 'http://127.0.0.1:1');
CREATE USER MAPPING FOR CURRENT_USER SERVER coverage_source OPTIONS (bearer_token 'fixture');
CREATE FOREIGN TABLE lake_unproven_view(url text, url_lower text)
 SERVER coverage_source OPTIONS (table 'events', lake_index 'regression_coverage');
DO $view$
DECLARE plan jsonb; remote text;
BEGIN
 EXECUTE $q$EXPLAIN (FORMAT JSON) SELECT * FROM lake_unproven_view WHERE url_lower LIKE '%evil.test%'$q$ INTO plan;
 remote := plan->0->'Plan'->>'ClickHouse SQL';
 IF remote LIKE '%_path%' THEN
  RAISE EXCEPTION 'column suffix alone must not prove an indexed view: %', remote;
 END IF;
END $view$;

DO $$
DECLARE
    query text;
    actual text;
BEGIN
    query := wc_lake_sql('regression_coverage', 'lake_current', '_path', '*evil.test*');
    EXECUTE 'SELECT string_agg(_path, '','' ORDER BY _path) FROM (' || query || ') q' INTO actual;
    IF actual IS DISTINCT FROM 'new,unindexed' THEN
        RAISE EXCEPTION 'appended match/exact recheck: expected new,unindexed, got %', actual;
    END IF;

    DELETE FROM lake_current WHERE _path = 'unindexed';
    UPDATE wc_lake_file SET has_unindexed = false WHERE index_name = 'regression_coverage';
    query := wc_lake_sql('regression_coverage', 'lake_current', '_path', '*evil.test*');
    IF query IS NULL THEN
        RAISE EXCEPTION 'zero indexed candidates must still query unknown files';
    END IF;
    EXECUTE query INTO actual;
    IF actual IS DISTINCT FROM 'new' THEN
        RAISE EXCEPTION 'zero indexed candidates: expected new, got %', actual;
    END IF;

    DELETE FROM wc_lake_file WHERE index_name = 'regression_coverage';
    query := wc_lake_sql('regression_coverage', 'lake_current', '_path', '*evil.test*');
    IF query IS NULL THEN
        RAISE EXCEPTION 'empty inventory must still query unknown files';
    END IF;
    EXECUTE query INTO actual;
    IF actual IS DISTINCT FROM 'new' THEN
        RAISE EXCEPTION 'empty inventory: expected new, got %', actual;
    END IF;

    INSERT INTO wc_lake_file VALUES ('regression_coverage', 0, repeat('x', 65536), 1, false);
    query := wc_lake_sql('regression_coverage', 'lake_current', 'url', '*evil.test*',
                         'wildcard', 'raw', '_path = ''new''');
    IF query LIKE '%NOT IN%' THEN
        RAISE EXCEPTION 'oversized exclusion guard must widen';
    END IF;
    EXECUTE query INTO actual;
    IF actual IS DISTINCT FROM 'evil.test' THEN
        RAISE EXCEPTION 'budget fallback must retain exact predicate and extra restriction';
    END IF;
END
$$;

-- Planning a selective query over 10,000 immutable paths stays compact and
-- performs no network request (coverage_source deliberately has no endpoint).
INSERT INTO wc_lake_index(index_name, column_name, views, snapshot_id)
VALUES ('regression_scale', 'url', 'raw', 2);
INSERT INTO wc_lake_file
SELECT 'regression_scale', n, 's3://lake/data/' || lpad(n::text, 8, '0') || '.parquet', 1, n = 0
FROM generate_series(0, 9999) AS n;
CREATE FOREIGN TABLE lake_scaled(url text)
 SERVER coverage_source OPTIONS (table 'events', lake_index 'regression_scale', lake_known_table 'wc_lake_known');
DO $scale$
DECLARE plan jsonb; remote text; reason text;
BEGIN
 EXECUTE $q$EXPLAIN (FORMAT JSON) SELECT * FROM lake_scaled WHERE wc_match(url, '*needle*')$q$ INTO plan;
 remote := plan->0->'Plan'->>'ClickHouse SQL';
 reason := plan->0->'Plan'->>'Lake files';
 IF remote NOT LIKE '%SELECT path FROM wc_lake_known WHERE index_name = ''wc-lake-v1:%'
    OR length(remote) > 1024 OR reason NOT LIKE '%server inventory%' THEN
  RAISE EXCEPTION 'expected compact coverage-bound server inventory: %, %', remote, reason;
 END IF;
 ALTER FOREIGN TABLE lake_scaled OPTIONS (DROP lake_known_table);
 EXECUTE $q$EXPLAIN (FORMAT JSON) SELECT * FROM lake_scaled WHERE wc_match(url, '*needle*')$q$ INTO plan;
 remote := plan->0->'Plan'->>'ClickHouse SQL';
 reason := plan->0->'Plan'->>'Lake files';
 IF remote LIKE '%_path%' OR reason NOT LIKE '%exceeds 64 KiB%' THEN
  RAISE EXCEPTION 'expected visible conservative overflow: %, %', remote, reason;
 END IF;
END $scale$;

ROLLBACK;
