-- SPDX-License-Identifier: Apache-2.0
-- Run as a superuser with psql -X -v ON_ERROR_STOP=1 in an owned fixture.
BEGIN;
CREATE EXTENSION IF NOT EXISTS pg_wildcard;
CREATE EXTENSION IF NOT EXISTS file_fdw;
CREATE SCHEMA planner_identity;
CREATE SERVER planner_identity_file FOREIGN DATA WRAPPER file_fdw;
CREATE FOREIGN TABLE planner_identity.foreign_values(v text, v_lower text)
  SERVER planner_identity_file
  OPTIONS (program 'printf "EVIL,unrelated\ngood,evil\n"', format 'csv');
CREATE TABLE planner_identity.local_values(v text);
INSERT INTO planner_identity.local_values VALUES ('evil'), ('good');
CREATE FUNCTION planner_identity.re2match(text,text) RETURNS boolean
  LANGUAGE plpgsql IMMUTABLE STRICT AS 'BEGIN RETURN false; END';
SET LOCAL search_path = planner_identity, public, pg_catalog;

DO $test$
DECLARE
  result text[];
BEGIN
  SELECT array_agg(v ORDER BY v) INTO result FROM local_values
    WHERE wc_regex(v, '^evil$', false, 'raw');
  IF result IS DISTINCT FROM ARRAY['evil'] THEN
    RAISE EXCEPTION 'a search_path function replaced the regex implementation: %', result;
  END IF;
  SELECT array_agg(v ORDER BY v) INTO result FROM foreign_values
    WHERE wc_match(v, 'evil', 'lower');
  IF result IS DISTINCT FROM ARRAY['EVIL'] THEN
    RAISE EXCEPTION 'a same-named foreign column replaced the actual view: %', result;
  END IF;
  SELECT array_agg(v ORDER BY v) INTO result FROM foreign_values
    WHERE wc_like(v, 'evil', 'lower');
  IF result IS DISTINCT FROM ARRAY['EVIL'] THEN
    RAISE EXCEPTION 'LIKE used an unproven materialized foreign column: %', result;
  END IF;
END $test$;
ROLLBACK;
