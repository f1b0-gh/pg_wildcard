#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Read-only capability diagnostics and non-executable migration review plans."""
import argparse
import base64
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import urllib.parse
import urllib.request

PINNED_VERSION = "26.8.34.20001.altinityantalya"
PINNED_IMAGE = "sha256:0989fbcd21552463cff9ef8d7a44bd2213a2c8e1879816f856a744a14b28e77a"
LAYOUT_KEYS = ("index_granularity", "index_granularity_bytes",
               "min_compress_block_size", "max_compress_block_size")
FUNCTIONS = ("match", "like", "lower", "decodeURLComponent")
MAX_BYTES = 2 * 1024 * 1024


class Refusal(Exception):
    """Messages are fixed, safe reason codes; never include remote text."""


def ident(value):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", value):
        raise Refusal("invalid_identifier")
    return "`" + value + "`"


def literal_identifier(value):
    ident(value)
    return "'" + value + "'"


def parse_profile(ddl):
    if len(ddl) > MAX_BYTES:
        raise Refusal("profile_too_large")
    creates = [line for line in ddl.splitlines() if line.startswith("CREATE TABLE ")]
    prefix = "CREATE TABLE logs (url String) ENGINE = MergeTree ORDER BY tuple() SETTINGS "
    if len(creates) != 1 or not creates[0].startswith(prefix) or not creates[0].endswith(";"):
        raise Refusal("canonical_profile_format_changed")
    settings = {}
    for pair in creates[0][len(prefix):-1].split(", "):
        match = re.fullmatch(r"([a-z_]+) = ([0-9]{1,10})", pair)
        if not match or match[1] in settings or match[1] not in LAYOUT_KEYS:
            raise Refusal("canonical_profile_format_changed")
        settings[match[1]] = int(match[2])
    if set(settings) != set(LAYOUT_KEYS) or any(v <= 0 for v in settings.values()):
        raise Refusal("canonical_profile_format_changed")
    return settings


def canonical_profile(executable):
    # The executable is an operator-selected, trusted local build, never remote input.
    try:
        result = subprocess.run([str(Path(executable).resolve()), "--selective-layout"],
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                timeout=30, check=True)
        return parse_profile(result.stdout.decode("utf-8"))
    except (OSError, UnicodeError, subprocess.SubprocessError):
        raise Refusal("canonical_profile_emitter_failed") from None


def load_config(path):
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                raise Refusal("config_requires_owner_only_regular_file")
            raw = stream.read(65537)
        if len(raw) > 65536:
            raise Refusal("config_too_large")
        config = json.loads(raw)
        if not isinstance(config, dict) or set(config) - {"endpoint", "user", "password", "bearer", "allow_loopback_http"}:
            raise Refusal("invalid_config")
        return config
    except (OSError, ValueError, RecursionError):
        raise Refusal("config_unreadable_or_invalid") from None


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        raise Refusal("http_redirect_refused")


class Client:
    def __init__(self, config):
        try:
            if not isinstance(config.get("endpoint"), str):
                raise Refusal("invalid_config")
            url = urllib.parse.urlsplit(config["endpoint"])
            loopback = url.hostname in ("127.0.0.1", "::1", "localhost")
            if (url.username or url.password or url.query or url.fragment or not url.hostname
                    or url.path not in ("", "/") or url.port == 0
                    or not (url.scheme == "https" or (url.scheme == "http" and loopback
                            and config.get("allow_loopback_http") is True))):
                raise Refusal("endpoint_requires_https_without_credentials_or_query")
            self.endpoint = config["endpoint"]
            self.headers = {"Content-Type": "text/plain; charset=utf-8"}
            if "bearer" in config:
                if "user" in config or "password" in config:
                    raise Refusal("ambiguous_authentication")
                token = config["bearer"]
                if not isinstance(token, str) or not token or not re.fullmatch(r"[A-Za-z0-9._~+/-]+=*", token):
                    raise Refusal("invalid_authentication")
                self.headers["Authorization"] = "Bearer " + token
            elif "user" in config or "password" in config:
                user, password = config["user"], config["password"]
                if not isinstance(user, str) or not isinstance(password, str) or ":" in user:
                    raise Refusal("invalid_authentication")
                auth = base64.b64encode((user + ":" + password).encode()).decode()
                self.headers["Authorization"] = "Basic " + auth
        except (KeyError, TypeError, ValueError):
            raise Refusal("invalid_config") from None
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def query(self, sql):
        if not sql.startswith("SELECT ") or ";" in sql:
            raise Refusal("read_only_query_required")
        # readonly is sent as a setting before query execution, including on failures.
        endpoint = self.endpoint.rstrip("/") + "/?readonly=1&max_execution_time=15&max_result_rows=10000&result_overflow_mode=throw"
        request = urllib.request.Request(endpoint, data=(sql + " FORMAT JSON").encode(), headers=self.headers)
        try:
            with self.opener.open(request, timeout=20) as response:
                raw = response.read(MAX_BYTES + 1)
            if len(raw) > MAX_BYTES:
                raise Refusal("diagnostic_response_too_large")
            payload = json.loads(raw)
            rows = payload["data"]
            if not isinstance(rows, list) or len(rows) > 10000 or any(not isinstance(r, dict) for r in rows):
                raise Refusal("invalid_diagnostic_response")
            return rows
        except (OSError, ValueError, KeyError, TypeError, RecursionError, http.client.HTTPException):
            raise Refusal("diagnostic_query_failed_check_access_and_engine_capabilities") from None


def layout_from_ddl(ddl):
    """Read only top-level numeric SETTINGS, ignoring quoted expressions."""
    if len(ddl) > MAX_BYTES:
        raise Refusal("diagnostic_response_too_large")
    quote, depth, start, index = None, 0, None, 0
    entries = []
    while index < len(ddl):
        character = ddl[index]
        if quote:
            if character == "\\":
                index += 2
                continue
            if character == quote:
                if index + 1 < len(ddl) and ddl[index + 1] == quote:
                    index += 2
                    continue
                quote = None
        elif character in "'\"`":
            quote = character
        elif character == "(":
            depth += 1
        elif character == ")":
            depth -= 1
        elif depth == 0:
            if ddl[index:index + 8] == "SETTINGS" and (index == 0 or ddl[index - 1].isspace()) and (index + 8 == len(ddl) or ddl[index + 8].isspace()):
                start = index + 8
                entries = []
                index += 8
                continue
            if character == "," and start is not None:
                entries.append(ddl[start:index])
                start = index + 1
        index += 1
    layout = dict.fromkeys(LAYOUT_KEYS)
    if quote or depth != 0:
        return layout
    if start is not None:
        entries.append(ddl[start:])
    for entry in entries:
        match = re.fullmatch(r"\s*([a-z_]+)\s*=\s*([0-9]{1,10})\s*", entry)
        if match and match[1] in layout:
            layout[match[1]] = int(match[2])
    return layout


def collect(client, database, table):
    db, tab = literal_identifier(database), literal_identifier(table)
    where = f"database = {db} AND table = {tab}"
    tables = client.query(f"SELECT engine, uuid, create_table_query FROM system.tables WHERE database = {db} AND name = {tab}")
    if len(tables) != 1:
        raise Refusal("table_missing_or_not_visible")
    ddl = tables[0]["create_table_query"]
    version = client.query("SELECT version() AS version")[0]["version"]
    databases = client.query(f"SELECT engine FROM system.databases WHERE name = {db}")
    columns = client.query(f"SELECT name, type, default_kind FROM system.columns WHERE {where} ORDER BY position")
    functions = client.query("SELECT name FROM system.functions WHERE name IN ('match','like','lower','decodeURLComponent')")
    settings = client.query("SELECT name, value FROM system.settings WHERE name IN ('framing_output_format','allow_experimental_full_text_index','use_skip_indexes','use_skip_indexes_on_data_read','use_query_condition_cache')")
    indexes = client.query(f"SELECT name, type, granularity, data_compressed_bytes FROM system.data_skipping_indices WHERE {where}")
    mutations = client.query(f"SELECT count() AS n FROM system.mutations WHERE {where} AND NOT is_done")
    # Only expose scalar numeric layout values, never expressions/comments/paths.
    layout = layout_from_ddl(ddl)
    return {"version": version, "database_engine": databases[0]["engine"] if len(databases) == 1 else None,
            "create_table_query": ddl,
            "engine": tables[0]["engine"], "table_uuid": tables[0]["uuid"], "schema_sha256": hashlib.sha256(ddl.encode()).hexdigest(),
            "columns": columns, "functions": [row["name"] for row in functions],
            "settings": {row["name"]: row["value"] for row in settings}, "layout": layout,
            "indexes": indexes, "pending_mutations": int(mutations[0]["n"])}


def diagnose(snapshot):
    blockers, warnings = [], []
    if snapshot["version"] != PINNED_VERSION:
        blockers.append("engine_version_unqualified")
    if "framing_output_format" not in snapshot["settings"]:
        blockers.append("missing_in_band_framing")
    for function in FUNCTIONS:
        if function not in snapshot["functions"]:
            blockers.append("missing_function_" + function)
    if not any(i.get("type", "").lower() == "text" for i in snapshot["indexes"]):
        warnings.append("no_text_index_definition")
    if any(v is None for v in snapshot["layout"].values()):
        warnings.append("layout_not_fully_explicit_review_server_defaults")
    fields = ("version", "database_engine", "engine", "table_uuid", "schema_sha256", "columns", "functions",
              "settings", "layout", "indexes", "pending_mutations")
    return {**{key: snapshot[key] for key in fields}, "blockers": blockers, "warnings": warnings,
            "qualified_version": PINNED_VERSION, "qualified_image": PINNED_IMAGE,
            "image_digest_verified": False, "readiness": "blocked" if blockers else "metadata_checks_passed",
            "index_materialization": "unknown", "query_pruning": "not_observed",
            "lake_snapshot": "not_observed", "postgres_extensions": "not_observed",
            "note": "Metadata observations do not prove query execution, index coverage, image identity or production readiness."}


def candidate_ddl(snapshot, database, table, candidate, profile):
    """Let the pinned engine clone its schema; never interpolate opaque DDL."""
    if set(profile) != set(LAYOUT_KEYS) or any(type(v) is not int or not 0 < v < 2**32 for v in profile.values()):
        raise Refusal("invalid_canonical_profile")
    settings_sql = ", ".join(key + " = " + str(profile[key]) for key in LAYOUT_KEYS)
    return (f"CREATE TABLE {ident(database)}.{ident(candidate)} AS "
            + f"{ident(database)}.{ident(table)} SETTINGS {settings_sql};")


def migration_plan(snapshot, database, table, candidate, profile):
    source = ident(database) + "." + ident(table)
    target = ident(database) + "." + ident(candidate)
    if source == target:
        raise Refusal("candidate_must_differ")
    if diagnose(snapshot)["blockers"]:
        raise Refusal("capability_checks_failed")
    if snapshot["engine"] != "MergeTree" or snapshot["database_engine"] != "Atomic":
        raise Refusal("only_plain_mergetree_in_atomic_database_supported")
    if snapshot["pending_mutations"] != 0:
        raise Refusal("pending_mutations_refused")
    columns = snapshot["columns"]
    if not columns or len(columns) > 512:
        raise Refusal("unsupported_schema")
    # Explicit columns include MATERIALIZED/ALIAS in validation, but not insertion.
    names = [ident(c["name"]) for c in columns]
    if len(set(names)) != len(names):
        raise Refusal("unsupported_schema")
    if any(c["default_kind"] not in ("", "DEFAULT", "MATERIALIZED", "ALIAS") for c in columns):
        raise Refusal("unsupported_computed_column")
    writable = [ident(c["name"]) for c in columns if c["default_kind"] in ("", "DEFAULT")]
    if not writable:
        raise Refusal("no_writable_columns")
    all_columns, insert_columns = ", ".join(names), ", ".join(writable)
    left, right = f"SELECT {all_columns} FROM {source}", f"SELECT {all_columns} FROM {target}"
    exchange = f"EXCHANGE TABLES {source} AND {target};"
    metadata_checks = {}
    for system_table, fields, name_field in (
        ("columns", "name, type, default_kind, default_expression, compression_codec", "table"),
        ("data_skipping_indices", "name, type_full, expr, granularity", "table"),
        ("tables", "engine, partition_key, sorting_key, primary_key, sampling_key", "name"),
    ):
        base = f"SELECT {fields} FROM system.{system_table} WHERE database = {literal_identifier(database)} AND {name_field} = "
        a, b = base + literal_identifier(table), base + literal_identifier(candidate)
        metadata_checks[system_table] = f"SELECT count() AS differing_metadata FROM (({a} EXCEPT ALL {b}) UNION ALL ({b} EXCEPT ALL {a}));"
    return {
        "kind": "review_only_selective_migration", "executable": False,
        "source_schema_sha256": snapshot["schema_sha256"], "source_uuid": snapshot["table_uuid"], "canonical_layout": profile,
        "create_sql": candidate_ddl(snapshot, database, table, candidate, profile),
        "creation_instruction": "The pinned engine clones the source definition and overrides only canonical layout settings. Review SHOW CREATE before/after, including constraints, derived columns, indexes, codecs, TTL, keys, projections and all other settings. The candidate gets a new UUID; grants and row policies need separate review. Never ALTER index_granularity.",
        "required_gates": ["trusted_image_digest_verified", "full_source_visibility_for_copy",
                           "security_review", "dependencies_and_ttl_review", "candidate_absent_before_creation",
                           "reviewed_candidate_ddl", "freeze_and_drain_all_writers", "no_pending_mutations",
                           "source_uuid_and_schema_unchanged", "candidate_empty_before_copy", "copy_completed_once",
                           "exact_multiset_equal", "per_role_results_and_denials_equal",
                           "index_and_layout_verified", "cutover_authorized"],
        "copy_once_sql": f"INSERT INTO {target} ({insert_columns}) SELECT {insert_columns} FROM {source};",
        "verify_multiset_sql": f"SELECT count() AS differing_rows FROM (({left} EXCEPT ALL {right}) UNION ALL ({right} EXCEPT ALL {left}));",
        "verify_metadata_sql": metadata_checks,
        "cutover_sql": exchange, "rollback_before_writes_sql": exchange,
        "rollback_after_writes_sql": None,
        "rollback_instruction": "After exchange keep writers paused; verify UUID/name mapping and per-role access. Exchange back only if the first exchange succeeded and no writes occurred. After writes resume, freeze again and reconcile all new writes before any reverse exchange.",
        "failure_instruction": "On any error keep source authoritative and writers paused. Do not retry INSERT into a partly filled candidate. Inspect query completion and candidate UUID/contents; rebuild a fresh reviewed candidate. Do not run cutover or cleanup automatically.",
        "security_instruction": "Review grants, role inheritance and row policies for BOTH table names/UUIDs, including wildcard grants. Restrict candidate access before copying. Apply and test policy equivalents under each role. EXCHANGE is not proof of preserved authorization. Never print grants or policy expressions in shared diagnostics.",
        "retention_instruction": "Retain the old table and protected evidence until the rollback window closes. No DROP command is generated.",
    }


class SafeParser(argparse.ArgumentParser):
    def error(self, message):
        self.exit(2, "invalid arguments; use --help (credentials belong only in protected config)\n")


def main(argv=None):
    parser = SafeParser(description=__doc__)
    parser.add_argument("command", choices=("diagnose", "plan"))
    parser.add_argument("--config", required=True, help="owner-only JSON connection file")
    parser.add_argument("--database", required=True)
    parser.add_argument("--table", required=True)
    parser.add_argument("--candidate")
    parser.add_argument("--profile-emitter", help="trusted compiled libwildcard print_ddl example")
    args = parser.parse_args(argv)
    try:
        ident(args.database)
        ident(args.table)
        if args.command == "plan" and (not args.candidate or not args.profile_emitter):
            raise Refusal("plan_requires_candidate_and_profile_emitter")
        if args.candidate:
            ident(args.candidate)
        client = Client(load_config(args.config))
        snapshot = collect(client, args.database, args.table)
        if args.command == "plan":
            existing = client.query(f"SELECT count() AS n FROM system.tables WHERE database = {literal_identifier(args.database)} AND name = {literal_identifier(args.candidate)}")
            if int(existing[0]["n"]) != 0:
                raise Refusal("candidate_already_exists")
            result = migration_plan(snapshot, args.database, args.table, args.candidate,
                                    canonical_profile(args.profile_emitter))
        else:
            result = diagnose(snapshot)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 2 if result.get("blockers") else 0
    except Refusal as error:
        print(json.dumps({"status": "refused", "reason": str(error)}))
        return 2
    except (KeyError, TypeError, ValueError, IndexError):
        print(json.dumps({"status": "refused", "reason": "malformed_diagnostic_metadata"}))
        return 2


if __name__ == "__main__":
    sys.exit(main())
