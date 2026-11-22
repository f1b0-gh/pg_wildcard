-- SPDX-License-Identifier: Apache-2.0
-- Offline planner regressions; no remote service is contacted.
BEGIN;
CREATE EXTENSION IF NOT EXISTS pg_wildcard;
CREATE SERVER predicate_review FOREIGN DATA WRAPPER wildcard_fdw OPTIONS (url 'http://127.0.0.1:1');
CREATE USER MAPPING FOR CURRENT_USER SERVER predicate_review OPTIONS (bearer_token 'fixture');
CREATE FOREIGN TABLE predicate_values(v text, ts timestamp, tz timestamptz, d date)
 SERVER predicate_review OPTIONS (table 'events');
DO $test$
DECLARE plan jsonb; remote text; q text; style text; expected text;
BEGIN
 FOREACH q IN ARRAY ARRAY[
  $q$SELECT v FROM predicate_values WHERE v LIKE E'a\\bc'$q$,
  $q$SELECT v FROM predicate_values WHERE v NOT LIKE E'a\\bc'$q$
 ] LOOP
  EXECUTE 'EXPLAIN (FORMAT JSON) ' || q INTO plan;
  remote := plan->0->'Plan'->>'ClickHouse SQL';
  IF remote NOT LIKE '%LIKE ''abc''%' THEN
   RAISE EXCEPTION 'LIKE must preserve PostgreSQL escape semantics: %', remote;
  END IF;
 END LOOP;
 FOREACH style IN ARRAY ARRAY['ISO, MDY', 'SQL, MDY', 'SQL, DMY', 'German, DMY'] LOOP
  PERFORM set_config('DateStyle', style, true);
  PERFORM set_config('TimeZone', 'America/New_York', true);
  FOR q, expected IN SELECT * FROM (VALUES
   ($q$SELECT v FROM predicate_values WHERE ts = TIMESTAMP '2026-11-05 12:34:56.123456'$q$, 'fromUnixTimestamp64Micro(1793882096123456, ''UTC'')'),
   ($q$SELECT v FROM predicate_values WHERE tz = TIMESTAMPTZ '2026-11-05 07:34:56.123456-05'$q$, 'fromUnixTimestamp64Micro(1793882096123456, ''UTC'')'),
   ($q$SELECT v FROM predicate_values WHERE d = DATE '2026-11-05'$q$, 'toDate32(''2026-11-05'')')
  ) cases(q, expected) LOOP
   EXECUTE 'EXPLAIN (FORMAT JSON) ' || q INTO plan;
   remote := plan->0->'Plan'->>'ClickHouse SQL';
   IF position(expected IN remote) = 0 THEN
    RAISE EXCEPTION 'temporal constant depends on session settings: % %', style, remote;
   END IF;
  END LOOP;
 END LOOP;
 FOREACH q IN ARRAY ARRAY[
  $q$SELECT v FROM predicate_values WHERE ts = TIMESTAMPTZ '2026-11-05 07:34:56-05'$q$,
  $q$SELECT v FROM predicate_values WHERE ts = TIMESTAMP 'infinity'$q$,
  $q$SELECT v FROM predicate_values WHERE d = DATE '2500-01-01'$q$,
  $q$SELECT v FROM predicate_values WHERE v LIKE E'abc\\'$q$
 ] LOOP
  EXECUTE 'EXPLAIN (FORMAT JSON) ' || q INTO plan;
  remote := plan->0->'Plan'->>'ClickHouse SQL';
  IF remote LIKE '% WHERE %' THEN
   RAISE EXCEPTION 'unsafe temporal or invalid LIKE comparison must stay local: %', remote;
  END IF;
 END LOOP;
END $test$;
ROLLBACK;
