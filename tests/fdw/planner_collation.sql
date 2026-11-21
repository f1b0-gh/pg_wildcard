-- SPDX-License-Identifier: Apache-2.0
-- PG18 ICU regression: unsafe predicates must not narrow remotely or in Puffin.
BEGIN;
CREATE EXTENSION IF NOT EXISTS pg_wildcard;
CREATE SCHEMA next_collation;
CREATE COLLATION next_collation.nocase (provider=icu, locale='und-u-ks-level2', deterministic=false);
CREATE COLLATION next_collation.locale_order (provider=icu, locale='en-US', deterministic=true);
CREATE SERVER next_collation_server FOREIGN DATA WRAPPER wildcard_fdw OPTIONS (url 'http://127.0.0.1:1');
CREATE USER MAPPING FOR CURRENT_USER SERVER next_collation_server OPTIONS (bearer_token 'planner-no-network');
CREATE FOREIGN TABLE next_collation.events(id bigint, v text COLLATE next_collation.nocase)
  SERVER next_collation_server OPTIONS (table 'events', candidate_columns 'v', candidate_prewhere 'true');
CREATE FOREIGN TABLE next_collation.ordered(id bigint, v text COLLATE next_collation.locale_order)
  SERVER next_collation_server OPTIONS (table 'events');
INSERT INTO wc_lake_index VALUES ('next_collation_lake', 'v', 'raw', 1);
INSERT INTO wc_lake_file VALUES ('next_collation_lake', 0, 's3://test/uppercase.parquet', 1, false);
-- No lowercase grams: a byte search may prune; ICU case-insensitive LIKE may not.
CREATE FOREIGN TABLE next_collation.lake(id bigint, v text COLLATE next_collation.nocase)
  SERVER next_collation_server OPTIONS (table 'events', lake_index 'next_collation_lake');
DO $test$
DECLARE q text; plan jsonb; scan jsonb; remote text;
BEGIN
  IF NOT ('EVIL' COLLATE next_collation.nocase LIKE '%evil%') THEN
    RAISE EXCEPTION 'fixture must exercise nondeterministic LIKE';
  END IF;
  FOREACH q IN ARRAY ARRAY[
    $$SELECT id FROM next_collation.events WHERE v LIKE '%evil%' ORDER BY id LIMIT 1$$,
    $$SELECT id FROM next_collation.events WHERE v NOT LIKE '%evil%' ORDER BY id LIMIT 1$$,
    $$SELECT id FROM next_collation.events WHERE v='evil' ORDER BY id LIMIT 1$$,
    $$SELECT id FROM next_collation.events WHERE v<>'evil' ORDER BY id LIMIT 1$$,
    $$SELECT id FROM next_collation.events WHERE v=('evil'::name COLLATE next_collation.nocase) ORDER BY id LIMIT 1$$,
    $$SELECT id FROM next_collation.ordered WHERE v<'Z' ORDER BY id LIMIT 1$$,
    $$SELECT id FROM next_collation.ordered WHERE v<('Z'::name COLLATE next_collation.locale_order) ORDER BY id LIMIT 1$$,
    $$SELECT id FROM next_collation.lake WHERE v LIKE '%evil%' ORDER BY id LIMIT 1$$
  ] LOOP
    EXECUTE 'EXPLAIN (FORMAT JSON) ' || q INTO plan;
    SELECT x INTO scan FROM jsonb_path_query(plan, '$.** ? (@."Node Type" == "Foreign Scan")') x LIMIT 1;
    remote := scan->>'ClickHouse SQL';
    IF remote IS NULL OR remote LIKE '%WHERE%' OR remote LIKE '% LIMIT %' OR scan ? 'Lake pruning' THEN
      RAISE EXCEPTION 'collation-sensitive predicate must remain local without candidate/lake pruning: %', plan;
    END IF;
  END LOOP;
END $test$;
ROLLBACK;
