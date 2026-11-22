#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Differential test: libwildcard (Rust) vs ClickHouse (embedded via chDB).

    pip install chdb
    # Run from the libwildcard directory:
    cargo run --locked -q --features hot --example difftest_gen -- 42 | python3 tests/differential/run.py

Exit code 0 only if every view, predicate, sweep and watchlist result agrees.
"""
import argparse, json, sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--antalya-container', help='Owned soc-http-test-* fixture; creates and removes a private test database/user')
args = parser.parse_args()
spec = json.load(sys.stdin)
if args.antalya_container:
    from native_backend import NativeSession
    s = NativeSession(args.antalya_container)
else:
    from chdb import session
    s = session.Session()
def q(sql):
    return s.query(sql, "JSONCompact")

def rows(sql):
    res = q(sql)
    return json.loads(res.data())["data"] if res.data().strip() else []

vals = spec["values_hex"]
q("CREATE DATABASE IF NOT EXISTS d; USE d")
q("DROP TABLE IF EXISTS t")
q("CREATE TABLE t (id UInt32, v String) ENGINE = MergeTree ORDER BY id")
q("INSERT INTO t VALUES " + ", ".join(f"({i}, unhex('{h}'))" for i, h in enumerate(vals)))

failures = 0
def fail(msg):
    global failures
    failures += 1
    if failures <= 25:
        print("FAIL", msg)

# 1. views
for v in spec["views"]:
    got = [r[0] for r in rows(f"SELECT hex({v['expr']}) FROM t ORDER BY id")]
    for i, (g, e) in enumerate(zip(got, v["expected"])):
        if g != e:
            fail(f"view {v['view']} value {bytes.fromhex(vals[i])!r}: clickhouse={bytes.fromhex(g)!r} rust={bytes.fromhex(e)!r}")
print(f"views: {len(spec['views'])} x {len(vals)} values checked")

# 2. predicates
n_ok = 0
for p in spec["predicates"]:
    try:
        got = sorted(r[0] for r in rows(f"SELECT id FROM t WHERE {p['sql']}"))
    except Exception as ex:
        fail(f"{p['kind']} {p['pattern']!r} [{p['view']}] SQL error: {str(ex)[:200]}")
        continue
    exp = sorted(p["expected"])
    if got != exp:
        extra = sorted(set(got) - set(exp)); missing = sorted(set(exp) - set(got))
        fail(f"{p['kind']} {p['pattern']!r} [{p['view']}] ch-only={[bytes.fromhex(vals[i]) for i in extra][:3]} rust-only={[bytes.fromhex(vals[i]) for i in missing][:3]}")
    else:
        n_ok += 1
print(f"predicates: {n_ok}/{len(spec['predicates'])} agree")

# 3. sweep
got = set()
for sql in spec["sweep"]["queries"]:
    for r in rows(sql):
        got.add((int(r[0]), int(r[1]), r[2]))
exp = {(r[0], r[1], r[2]) for r in spec["sweep"]["expected"]}
if got != exp:
    fail(f"sweep: ch-only={sorted(got-exp)[:5]} rust-only={sorted(exp-got)[:5]}")
print(f"sweep: {len(exp)} expected hits, {len(got)} from ClickHouse")

# Explicit same-view gate cases, including escaping and bounded fallbacks.
for case in spec.get("shared_gate_cases", []):
    got_gate = []
    for sql, estimate in zip(case["queries"], case["estimated_ast"]):
        if len(sql.encode()) > case["max_bytes"] or estimate > case["max_ast_nodes"]:
            fail(f"shared gate {case['name']}: budget exceeded")
        got_gate.extend((int(r[0]), int(r[1]), r[2]) for r in rows(sql))
    expected_gate = sorted(tuple(r) for r in case["expected"])
    if sorted(got_gate) != expected_gate:
        fail(f"shared gate {case['name']}: exact pairs differ")
print(f"shared gate: {len(spec.get('shared_gate_cases', []))} cases checked")

# 4. watchlist materialized view
w = spec["watchlist"]
q("DROP TABLE IF EXISTS watch"); q("DROP TABLE IF EXISTS src"); q("DROP TABLE IF EXISTS hits"); q("DROP VIEW IF EXISTS wc_mv")
q(w["table_ddl"])
for insert in w["inserts"]:
    q(insert)
q("CREATE TABLE src (id UInt32, v String) ENGINE = MergeTree ORDER BY id")
q("CREATE TABLE hits (id UInt32, indicator_id Int64, view String) ENGINE = MergeTree ORDER BY id")
q(w["mv"])
q("INSERT INTO src SELECT id, v FROM t")
got_w = {(int(r[0]), int(r[1]), r[2]) for r in rows("SELECT id, indicator_id, view FROM hits")}
if got_w != exp:
    fail(f"watchlist: ch-only={sorted(got_w-exp)[:5]} rust-only={sorted(exp-got_w)[:5]}")
print(f"watchlist: {len(got_w)} hits from materialized view")

# Regex-only and view-only writes must invalidate old derived metadata. All
# metadata defaults empty for legacy inserts, so direct writers also remain safe.
for label, projection in [
    ("stale regex", "id, view, '.*', needles, needles_regex, needles_view"),
    ("stale view", "id, 'raw', regex, needles, needles_regex, needles_view"),
    ("empty defaults", "id, view, regex"),
]:
    q("TRUNCATE TABLE watch")
    for insert in w["inserts"]:
        q(insert)
    q("CREATE TABLE saved ENGINE = Memory AS SELECT * FROM watch FINAL")
    q("TRUNCATE TABLE watch")
    columns = "id, view, regex" if label == "empty defaults" else "id, view, regex, needles, needles_regex, needles_view"
    q(f"INSERT INTO watch ({columns}) SELECT {projection} FROM saved")
    q("DROP TABLE saved")
    q("TRUNCATE TABLE hits")
    q("INSERT INTO src SELECT id, v FROM t")
    got_update = {(int(r[0]), int(r[1]), r[2]) for r in rows("SELECT id, indicator_id, view FROM hits")}
    # Independent exhaustive dynamic regex cross-product, no metadata gates.
    expressions = {v["view"]: v["expr"] for v in spec["views"]}
    expected_update = set()
    for view, expression in expressions.items():
        sql = f"SELECT t.id, w.id, w.view FROM t CROSS JOIN (SELECT * FROM watch FINAL WHERE active = 1 AND view = '{view}') w WHERE match({expression}, w.regex)"
        expected_update.update((int(r[0]), int(r[1]), r[2]) for r in rows(sql))
    if got_update != expected_update:
        fail(f"watchlist {label}: ch-only={sorted(got_update-expected_update)[:5]} exact-only={sorted(expected_update-got_update)[:5]}")
    print(f"watchlist {label}: {len(got_update)} hits, exact={len(expected_update)}")

print("RESULT:", "PASS" if failures == 0 else f"FAIL ({failures})")
sys.exit(1 if failures else 0)
