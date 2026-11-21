# SPDX-License-Identifier: Apache-2.0
"""Run with python3 -m unittest discover -s tests/operations -v."""
import importlib.util
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("wildcard_ops", ROOT / "tools/wildcard_ops.py")
ops = importlib.util.module_from_spec(spec) if spec else None
if spec and pathlib.Path(spec.origin).exists():
    spec.loader.exec_module(ops)


def profile():
    return {"index_granularity": 512, "index_granularity_bytes": 10485760,
            "min_compress_block_size": 16384, "max_compress_block_size": 65536}


def snapshot():
    return {
        "create_table_query": "CREATE TABLE db.events (id UInt64, raw String) ENGINE = MergeTree ORDER BY id SETTINGS index_granularity = 8192",
        "version": "26.8.34.20001.altinityantalya", "database_engine": "Atomic",
        "engine": "MergeTree", "table_uuid": "00000000-0000-0000-0000-000000000001", "schema_sha256": "a" * 64,
        "columns": [{"name": "id", "type": "UInt64", "default_kind": ""},
                    {"name": "raw", "type": "String", "default_kind": ""}],
        "functions": ["match", "like", "lower", "decodeURLComponent"],
        "settings": {"framing_output_format": ""},
        "layout": {"index_granularity": 8192, "index_granularity_bytes": 10485760,
                   "min_compress_block_size": 65536, "max_compress_block_size": 1048576},
        "indexes": [], "pending_mutations": 0,
    }


class OperationsTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(hasattr(ops, "migration_plan"), "operations tooling is not implemented")

    def test_identifiers_cannot_inject_or_accept_unbounded_input(self):
        for value in ("", "a.b", "x'; DROP TABLE x", "x\n", "a" * 129, "é", "a\0b"):
            with self.subTest(value=value[:20]), self.assertRaises(ops.Refusal):
                ops.ident(value)
        self.assertEqual(ops.ident("tenant_7"), "`tenant_7`")

    def test_profile_comes_from_emitter_and_rejects_extra_sql(self):
        ddl = "CREATE TABLE logs (url String) ENGINE = MergeTree ORDER BY tuple() SETTINGS index_granularity = 512, index_granularity_bytes = 10485760, min_compress_block_size = 16384, max_compress_block_size = 65536;\nALTER TABLE logs ADD COLUMN x String;"
        self.assertEqual(ops.parse_profile(ddl)["index_granularity"], 512)
        for bad in (ddl.replace("512,", "512; DROP TABLE x,", 1), ddl + "\n" + ddl,
                    ddl.replace("index_granularity_bytes", "unknown_setting")):
            with self.assertRaises(ops.Refusal):
                ops.parse_profile(bad)

    def test_incompatible_version_and_missing_capability_fail_closed(self):
        s = snapshot()
        s["version"] = "26.6.8.20004.altinityantalya"
        self.assertIn("engine_version_unqualified", ops.diagnose(s)["blockers"])
        s = snapshot()
        s["settings"] = {}
        self.assertIn("missing_in_band_framing", ops.diagnose(s)["blockers"])
        s["functions"].remove("match")
        self.assertIn("missing_function_match", ops.diagnose(s)["blockers"])

    def test_index_definition_never_claims_actual_query_usage_or_coverage(self):
        result = ops.diagnose(snapshot())
        self.assertEqual(result["index_materialization"], "unknown")
        self.assertEqual(result["query_pruning"], "not_observed")
        self.assertEqual(result["lake_snapshot"], "not_observed")
        self.assertIn("no_text_index_definition", result["warnings"])

    def test_layout_diagnostic_does_not_treat_column_literals_as_settings(self):
        ddl = "CREATE TABLE db.events (raw String DEFAULT 'SETTINGS index_granularity = 999,') ENGINE=MergeTree ORDER BY tuple() SETTINGS index_granularity = 512, storage_policy = 'index_granularity_bytes = 99,'"
        self.assertTrue(hasattr(ops, "layout_from_ddl"), "quote-aware settings reader is missing")
        layout = ops.layout_from_ddl(ddl)
        self.assertEqual(layout["index_granularity"], 512)
        self.assertIsNone(layout["index_granularity_bytes"])

    def test_plan_refuses_engines_mutations_and_aliasing(self):
        for key, value in (("engine", "ReplacingMergeTree"), ("database_engine", "Ordinary"),
                           ("pending_mutations", 1), ("columns", [])):
            s = snapshot()
            s[key] = value
            with self.subTest(key=key), self.assertRaises(ops.Refusal):
                ops.migration_plan(s, "db", "events", "candidate", {"index_granularity": 512})
        with self.assertRaises(ops.Refusal):
            ops.migration_plan(snapshot(), "db", "events", "events", {})

    def test_plan_generates_engine_clone_and_requires_security_review(self):
        plan = ops.migration_plan(snapshot(), "db", "events", "candidate", profile())
        self.assertFalse(plan["executable"])
        self.assertIn("CREATE TABLE `db`.`candidate`", plan["create_sql"])
        self.assertIn("AS `db`.`events` SETTINGS", plan["create_sql"])
        self.assertIn("security_review", plan["required_gates"])
        self.assertIn("freeze_and_drain_all_writers", plan["required_gates"])
        self.assertIn("EXCEPT ALL", plan["verify_multiset_sql"])
        self.assertIn("`raw`", plan["verify_multiset_sql"])
        self.assertEqual(plan["cutover_sql"], "EXCHANGE TABLES `db`.`events` AND `db`.`candidate`;")
        self.assertEqual(plan["rollback_before_writes_sql"], plan["cutover_sql"])
        self.assertIsNone(plan["rollback_after_writes_sql"])

    def test_copy_excludes_computed_columns_but_verifies_them(self):
        s = snapshot()
        s["columns"].append({"name": "derived", "type": "String", "default_kind": "MATERIALIZED"})
        s["indexes"] = [{"name": "idx", "type": "text"}]
        plan = ops.migration_plan(s, "db", "events", "candidate", profile())
        self.assertNotIn("derived", plan["copy_once_sql"])
        self.assertIn("derived", plan["verify_multiset_sql"])
        self.assertIn("AS `db`.`events`", plan["create_sql"])

    def test_opaque_source_ddl_is_never_interpolated_into_plan(self):
        for extra in (" TTL now()", "; DROP TABLE secret", " COMMENT 'secret'", ", storage_policy = 'secret'"):
            s = snapshot()
            s["create_table_query"] += extra
            plan = ops.migration_plan(s, "db", "events", "candidate", profile())
            self.assertNotIn(extra, plan["create_sql"])
            self.assertNotIn("secret", str(plan))

    def test_diagnostic_never_exports_raw_ddl_or_server_errors(self):
        s = snapshot()
        s["create_table_query"] = "secret_password_in_comment"
        s["server_error"] = "Authorization: Bearer secret"
        self.assertNotIn("secret", str(ops.diagnose(s)))

    def test_config_refuses_public_files_and_symlinks(self):
        with tempfile.TemporaryDirectory() as folder:
            path = pathlib.Path(folder) / "connection.json"
            path.write_text('{"endpoint":"https://example.invalid","password":"secret"}')
            path.chmod(0o644)
            with self.assertRaises(ops.Refusal):
                ops.load_config(path)
            path.chmod(0o600)
            self.assertEqual(ops.load_config(path)["endpoint"], "https://example.invalid")
            link = pathlib.Path(folder) / "link"
            link.symlink_to(path)
            with self.assertRaises(ops.Refusal):
                ops.load_config(link)

    def test_endpoint_refuses_embedded_credentials_cleartext_and_redirects(self):
        for endpoint in (123, "https://user:secret@example.invalid", "http://example.invalid",
                         "https://example.invalid/?password=secret", "file:///etc/passwd"):
            with self.subTest(endpoint=endpoint), self.assertRaises(ops.Refusal):
                ops.Client({"endpoint": endpoint})
        with self.assertRaises(ops.Refusal):
            ops.NoRedirect().redirect_request(None, None, None, None, None, None)

    def test_transport_is_read_only_bounded_and_rejects_late_failure(self):
        # Replace only the remote HTTP boundary: production Client parses its response.
        class Reply:
            def __init__(self, body):
                self.body = body
            def __enter__(self):
                return self
            def __exit__(self, *args):
                pass
            def read(self, size):
                return self.body[:size]
        class Remote:
            body = b'{"data":[{"version":"26.8.34.20001.altinityantalya"}]}'
            request = None
            def open(self, request, timeout):
                self.request = request
                return Reply(self.body)
        remote = Remote()
        client = ops.Client({"endpoint": "https://example.invalid"})
        client.opener = remote
        self.assertEqual(client.query("SELECT version() AS version")[0]["version"],
                         "26.8.34.20001.altinityantalya")
        self.assertIn("readonly=1", remote.request.full_url)
        self.assertEqual(remote.request.data, b"SELECT version() AS version FORMAT JSON")
        for body in (remote.body + b'\nserver failure secret_password', b"x" * (ops.MAX_BYTES + 1), b"[" * 10000):
            remote.body = body
            with self.assertRaises(ops.Refusal) as caught:
                client.query("SELECT 1")
            self.assertNotIn("secret", str(caught.exception))
        with self.assertRaises(ops.Refusal):
            client.query("DROP TABLE x")


if __name__ == "__main__":
    unittest.main()
