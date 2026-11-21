#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Owned Antalya fixture only. Called by antalya-rs/scripts/fixture.py --framed.

Arguments: --work <private directory> --profile-emitter <trusted print_ddl>
Optional --hold keeps fixture alive until <work>/release is created.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("wildcard_ops", ROOT / "tools/wildcard_ops.py")
ops = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ops)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", required=True)
    parser.add_argument("--profile-emitter", required=True)
    parser.add_argument("--hold", action="store_true")
    args = parser.parse_args()
    work = Path(args.work)
    info = work.lstat()
    assert stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and not work.is_symlink()
    container = os.environ["ANTALYA_TEST_CONTAINER"]
    assert container.startswith("antalya-rs-")
    assert os.environ["ANTALYA_TEST_VERSION"] == ops.PINNED_VERSION
    config = {"endpoint": os.environ["ANTALYA_TEST_URL"], "bearer": os.environ["ANTALYA_TEST_TOKEN"],
              "allow_loopback_http": True}
    client = ops.Client(config)
    path = work / "connection.json"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w") as file:
        json.dump(config, file)
    (work / "container-name").write_text(container)
    database = "ops_" + secrets.token_hex(6)
    user = database + "_reader"

    def sql(statement, *, principal=None, expected_failure=False):
        # Only this disposable container, never a caller-supplied external endpoint.
        command = ["docker", "exec", container, "clickhouse-client", "--max_threads", "1"]
        if principal:
            command += ["--user", principal]
        result = subprocess.run(command + ["--query", statement], capture_output=True, text=True)
        if expected_failure:
            assert result.returncode != 0, "denied query unexpectedly succeeded"
            return "denied"
        if result.returncode:
            raise RuntimeError("owned fixture SQL failed: " + result.stderr[:1000])
        return result.stdout.strip()

    results = []
    try:
        sql(f"CREATE DATABASE {database} ENGINE=Atomic")
        sql(f"CREATE TABLE {database}.events (id UInt64, raw String CODEC(ZSTD(1)), lowered String MATERIALIZED lower(raw), alias_raw String ALIAS raw, CONSTRAINT positive_id CHECK id > 0, INDEX raw_idx raw TYPE text(tokenizer=ngrams(3)) GRANULARITY 1, PROJECTION raw_projection (SELECT id, raw ORDER BY id)) ENGINE=MergeTree PARTITION BY id % 2 ORDER BY id SETTINGS index_granularity=8192, min_rows_for_wide_part=1234")
        sql(f"INSERT INTO {database}.events VALUES (1,'raw-A'),(1,'raw-A'),(2,'raw-B')")
        snapshot = ops.collect(client, database, "events")
        report = ops.diagnose(snapshot)
        assert report["readiness"] == "metadata_checks_passed", report["blockers"]
        results.append("read_only_pinned_engine_diagnostic")
        deadline = time.monotonic() + 180
        while not Path(args.profile_emitter).exists() and time.monotonic() < deadline:
            time.sleep(1)
        profile = ops.canonical_profile(args.profile_emitter)
        plan = ops.migration_plan(snapshot, database, "events", "candidate", profile)
        (work / "plan.json").write_text(json.dumps(plan, indent=2))
        sql(plan["create_sql"])
        sql(plan["copy_once_sql"])
        assert sql(plan["verify_multiset_sql"]) == "0"
        candidate = ops.collect(client, database, "candidate")
        assert candidate["layout"] == profile
        assert candidate["columns"] == snapshot["columns"]
        for statement in plan["verify_metadata_sql"].values():
            assert sql(statement) == "0"
        source_ddl = sql(f"SHOW CREATE TABLE {database}.events FORMAT TabSeparatedRaw")
        candidate_ddl = sql(f"SHOW CREATE TABLE {database}.candidate FORMAT TabSeparatedRaw")
        assert source_ddl.split("\n", 1)[1].split("\nSETTINGS")[0] == candidate_ddl.split("\n", 1)[1].split("\nSETTINGS")[0]
        assert "min_rows_for_wide_part = 1234" in candidate_ddl
        results.append("clone_preserves_indexes_derived_codecs_constraints_projection_partition_and_other_settings")
        results.append("create_copy_exact_duplicates_and_layout")
        sql(f"INSERT INTO {database}.candidate VALUES (1,'raw-A')")
        assert sql(plan["verify_multiset_sql"]) == "1"
        # Failure procedure uses a fresh empty candidate, never repeats INSERT.
        sql(f"DROP TABLE {database}.candidate SYNC")
        sql(plan["create_sql"])
        sql(plan["copy_once_sql"])
        assert sql(plan["verify_multiset_sql"]) == "0"
        results.append("duplicate_corruption_detected_and_rebuilt")
        sql(f"CREATE USER {user}")
        for table in ("events", "candidate"):
            sql(f"GRANT SELECT(id) ON {database}.{table} TO {user}")
            sql(f"CREATE ROW POLICY {database}_{table}_policy ON {database}.{table} USING id = 1 TO {user}")
        def check_security():
            for table in ("events", "candidate"):
                assert sql(f"SELECT id FROM {database}.{table} ORDER BY id", principal=user) == "1\n1"
                sql(f"SELECT raw FROM {database}.{table}", principal=user, expected_failure=True)
        check_security()
        original_uuid = sql(f"SELECT uuid FROM system.tables WHERE database='{database}' AND name='events'")
        sql(plan["cutover_sql"])
        check_security()
        assert sql(f"SELECT uuid FROM system.tables WHERE database='{database}' AND name='candidate'") == original_uuid
        sql(plan["rollback_before_writes_sql"])
        check_security()
        assert sql(f"SELECT uuid FROM system.tables WHERE database='{database}' AND name='events'") == original_uuid
        assert sql(plan["verify_multiset_sql"]) == "0"
        results.append("exchange_and_rollback_preserve_fixture_rows_grants_policies")
        sql(f"CREATE TABLE {database}.unsupported (id UInt64) ENGINE=ReplacingMergeTree ORDER BY id")
        try:
            ops.migration_plan(ops.collect(client, database, "unsupported"), database,
                               "unsupported", "candidate2", profile)
        except ops.Refusal:
            results.append("unsupported_engine_refused")
        else:
            raise AssertionError("unsupported migration accepted")
        (work / "native-results.json").write_text(json.dumps({"passed": results}, indent=2))
        print(json.dumps({"passed": results, "work": str(work)}), flush=True)
        if args.hold:
            while not (work / "release").exists():
                time.sleep(1)
    finally:
        sql(f"DROP DATABASE IF EXISTS {database} SYNC")
        sql(f"DROP USER IF EXISTS {user}")


if __name__ == "__main__":
    main()
