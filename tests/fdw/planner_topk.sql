-- SPDX-License-Identifier: Apache-2.0
-- Run with psql -X -v ON_ERROR_STOP=1 on the owned PG18.6 C.UTF-8 fixture.
-- Planning only: endpoint is deliberately unreachable, no native credentials.
BEGIN;
CREATE EXTENSION IF NOT EXISTS pg_wildcard;
CREATE SCHEMA next_topk;
CREATE SERVER next_topk_server FOREIGN DATA WRAPPER wildcard_fdw
  OPTIONS (url 'http://127.0.0.1:1');
CREATE USER MAPPING FOR CURRENT_USER SERVER next_topk_server
  OPTIONS (bearer_token 'planner-no-network');
CREATE FOREIGN TABLE next_topk.events(id bigint, v text, v_lower text)
  SERVER next_topk_server OPTIONS (table 'events');
CREATE TABLE next_topk.local_events(id bigint, v text);
CREATE FOREIGN DATA WRAPPER next_topk_alias HANDLER wildcard_fdw_handler;
CREATE SERVER next_topk_alias_server FOREIGN DATA WRAPPER next_topk_alias
  OPTIONS (url 'http://127.0.0.1:1');
CREATE USER MAPPING FOR CURRENT_USER SERVER next_topk_alias_server
  OPTIONS (bearer_token 'planner-no-network');
CREATE FOREIGN TABLE next_topk.alias_events(id bigint, v text)
  SERVER next_topk_alias_server OPTIONS (table 'events');
CREATE FUNCTION next_topk.wc_match(text,text,text) RETURNS boolean
  LANGUAGE plpgsql IMMUTABLE AS 'BEGIN RETURN true; END'
  SUPPORT wc_rewrite_support;
CREATE VIEW next_topk.visible WITH (security_barrier=true) AS
  SELECT * FROM next_topk.events WHERE id % 2 = 0;

DO $test$
DECLARE
  q text;
  plan jsonb;
  scan jsonb;
BEGIN
  -- wc_* owns byte/Unicode semantics independently of the database collation.
  FOREACH q IN ARRAY ARRAY[
    $$SELECT id,v FROM next_topk.events WHERE wc_match(v,'*event=*','raw') ORDER BY id LIMIT 32$$,
    $$SELECT id,v FROM next_topk.events e WHERE wc_like(e.v,'%event=%','raw') ORDER BY id LIMIT 32$$,
    $$SELECT id,v FROM next_topk.events WHERE wc_match(v,'*é?*','raw') ORDER BY id LIMIT 32$$,
    $$SELECT id,v FROM next_topk.alias_events WHERE wc_match(v,'*event=*','raw') ORDER BY id LIMIT 32$$,
    $$SELECT id,v FROM next_topk.events WHERE wc_match(v,'*event=*','lower') ORDER BY id LIMIT 32$$
  ] LOOP
    EXECUTE 'EXPLAIN (FORMAT JSON) ' || q INTO plan;
    SELECT x INTO scan FROM jsonb_path_query(plan, '$.** ? (@."Node Type" == "Foreign Scan")') x LIMIT 1;
    IF scan IS NULL OR NOT coalesce((scan->>'ClickHouse SQL') LIKE '% ORDER BY % LIMIT 32',false)
       OR NOT coalesce((scan->>'Filter') LIKE '%wc_%',false) THEN
      RAISE EXCEPTION 'extension predicate must retain local recheck and remote top-K: %', plan;
    END IF;
  END LOOP;
  EXECUTE $$EXPLAIN (FORMAT JSON) SELECT id FROM next_topk.events
    WHERE wc_match(v,'*event=*','raw') ORDER BY id LIMIT 7 OFFSET 3$$ INTO plan;
  SELECT x INTO scan FROM jsonb_path_query(plan, '$.** ? (@."Node Type" == "Foreign Scan")') x LIMIT 1;
  IF NOT coalesce((scan->>'ClickHouse SQL') LIKE '% LIMIT 10',false) OR (scan->>'ClickHouse SQL') LIKE '% OFFSET %' THEN
    RAISE EXCEPTION 'offset must be added to remote bound, not discarded twice: %', plan;
  END IF;

  -- Arbitrary LIKE collation and residual predicates must retain conservative fallback.
  FOREACH q IN ARRAY ARRAY[
    $$SELECT id FROM next_topk.events WHERE v LIKE '%event=%' ORDER BY id LIMIT 32$$,
    $$SELECT id FROM next_topk.events WHERE wc_match(v,'*event=*') AND length(v)>4 ORDER BY id LIMIT 32$$,
    $$SELECT id FROM next_topk.visible WHERE wc_match(v,'*event=*') ORDER BY id LIMIT 32$$,
    $$SELECT count(*) FROM next_topk.events WHERE wc_match(v,'*event=*')$$
  ] LOOP
    EXECUTE 'EXPLAIN (FORMAT JSON) ' || q INTO plan;
    SELECT x INTO scan FROM jsonb_path_query(plan, '$.** ? (@."Node Type" == "Foreign Scan")') x LIMIT 1;
    IF scan IS NULL OR NOT (scan ? 'ClickHouse SQL') OR (scan->>'ClickHouse SQL') LIKE '% LIMIT %' THEN
      RAISE EXCEPTION 'unsupported or collation-sensitive shape must not limit remotely: %', plan;
    END IF;
  END LOOP;
  EXECUTE $$EXPLAIN (FORMAT JSON) SELECT * FROM next_topk.local_events WHERE wc_match(v,'*é*')$$ INTO plan;
  IF plan::text NOT LIKE '%~~%' THEN
    RAISE EXCEPTION 'local table must retain existing index-friendly rewrite: %', plan;
  END IF;
  -- A same-named user function attached to the support hook is not our predicate.
  EXECUTE $$EXPLAIN (FORMAT JSON) SELECT id FROM next_topk.events
    WHERE next_topk.wc_match(v,'never-present','raw') ORDER BY id LIMIT 32$$ INTO plan;
  SELECT x INTO scan FROM jsonb_path_query(plan, '$.** ? (@."Node Type" == "Foreign Scan")') x LIMIT 1;
  IF scan IS NULL OR NOT (scan ? 'ClickHouse SQL') OR (scan->>'ClickHouse SQL') LIKE '% WHERE %'
     OR (scan->>'ClickHouse SQL') LIKE '% LIMIT %' THEN
    RAISE EXCEPTION 'same-name non-extension function must stay local: %', plan;
  END IF;
END $test$;
ROLLBACK;
