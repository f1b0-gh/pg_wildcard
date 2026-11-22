#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""End-to-end lake test: Iceberg table -> wc-lake Puffin index -> pruned ClickHouse query.

Checks, on a real Iceberg table read by ClickHouse's Iceberg engine (chDB):
 1. our Puffin file parses with an independent reader (pyiceberg's Footer model);
 2. pruned queries (`_path IN (...)`) return exactly the unpruned results;
 3. ClickHouse really skips non-candidate files: we corrupt one of them and the
    pruned query still succeeds, while the unpruned query fails.

    pip install chdb "pyiceberg[sql-sqlite,pyarrow]"
    # Run from the libwildcard directory:
    cargo build --locked --features cold --bin wc-lake && python3 tests/lake/iceberg_e2e.py target/debug/wc-lake
"""
import csv, hashlib, io, json, os, random, shutil, struct, subprocess, sys, tempfile
import pyarrow as pa
from pyiceberg.catalog.sql import SqlCatalog
from puffin_footer import read_footer
from chdb import session

wc_lake = sys.argv[1] if len(sys.argv) > 1 else "target/debug/wc-lake"
# chDB only reads local files under its working directory
root = tempfile.mkdtemp(prefix="wc_iceberg_", dir=os.getcwd())
wh = os.path.join(root, "warehouse")
os.makedirs(wh)
cat = SqlCatalog("t", uri=f"sqlite:///{root}/cat.db", warehouse=f"file://{wh}")
cat.create_namespace("sec")
schema = pa.schema([("id", pa.int64()), ("url", pa.string())])
tbl = cat.create_table("sec.http", schema=schema)

rnd = random.Random(7)
hosts = ["good.example", "shop.example", "cdn.example", "api.example"]
paths = ["/", "/index.html", "/search?q=x", "/img/a.png", "/static/app.js"]
rid = 0
for batch in range(10):  # each append = one data file
    rows = []
    for _ in range(500):
        rows.append((rid, f"https://{rnd.choice(hosts)}{rnd.choice(paths)}?b={batch}")); rid += 1
    if batch == 3:
        rows.append((rid, "https://LOGIN.EVIL.COM/%77p-admin/setup.PHP")); rid += 1
    if batch == 4:
        rows.append((rid, r"C:\Windows\System32\cmd.exe")); rid += 1
    if batch == 6:
        rows.append((rid, r"\\Server\Share\Tools\Tool.EXE")); rid += 1
    if batch == 2:
        for value in ["abcdgh", "abd", "run0.exe", "ab0z"]:
            rows.append((rid, value)); rid += 1
    if batch == 7:
        for value in ["abefgh", "acd", "run3.exe", "xy9z"]:
            rows.append((rid, value)); rid += 1
    if batch == 8:
        rows.append((rid, "http://evil.com.attacker.net/%2e%2e/etc/passwd")); rid += 1
    tbl.append(pa.table({"id": [r[0] for r in rows], "url": [r[1] for r in rows]}, schema=schema))
tbl = cat.load_table("sec.http")
snap = tbl.current_snapshot()
files = [t.file.file_path for t in tbl.scan().plan_files()]
assert len(files) == 10, files
table_dir = tbl.location().removeprefix("file://")
field_id = tbl.schema().find_field("url").field_id

s = session.Session()
# what does ClickHouse report as _path for these files?
ch_paths = sorted(r[0] for r in json.loads(s.query(f"SELECT DISTINCT _path FROM icebergLocal('{table_dir}')", "JSONCompact").data())["data"])
def local(p): return p.removeprefix("file://")
lake_by_local = {}
for f in files:
    match = [c for c in ch_paths if local(f).endswith(c.lstrip("/")) or c.endswith(os.path.basename(local(f)))]
    assert len(match) == 1, (f, ch_paths)
    lake_by_local[local(f)] = match[0]

idx = os.path.join(root, "http-url.puffin")
args = [wc_lake, "index", "--column", "url", "--views", "raw,decoded,revdomain,winpath", "--snapshot-id", str(snap.snapshot_id),
        "--sequence-number", str(snap.sequence_number), "--field-id", str(field_id), "--out", idx]
args += [f"{l}={c}" for l, c in lake_by_local.items()]
subprocess.run(args, check=True)

# 1. independent Puffin parse
raw = open(idx, "rb").read()
footer = read_footer(raw)
assert footer.blobs[0].type == "wildcard-ngram-v1" and footer.blobs[0].snapshot_id == snap.snapshot_id
print("puffin: PyIceberg metadata with explicit wildcard discriminator:", footer.blobs[0].type, dict(footer.blobs[0].properties))

def rows(sql):
    return json.loads(s.query(sql, "JSONCompact").data())["data"]

# Optional compiled fixture grows the local inventory to 10,000 immutable
# paths; added paths represent empty historical files absent from this snapshot.
server_idx = idx
if len(sys.argv) > 2:
    server_idx = os.path.join(root, "scaled.puffin")
    subprocess.run([sys.argv[2], idx, server_idx], check=True)

# The CLI exports an actual loader, keyed by the exact inventory digest. A
# different generation may be a superset; it must never hide an unknown file.
export_dir = os.path.join(root, "export")
subprocess.run([wc_lake, "export-pg", server_idx, "--name", "http_url", "--out-dir", export_dir, "--large-inventory", "on"], check=True)
known_schema = open(os.path.join(export_dir, "known-schema.sql")).read()
known_insert = open(os.path.join(export_dir, "known-insert.sql")).read()
known_csv = open(os.path.join(export_dir, "known.csv")).read()
coverage_id = open(os.path.join(export_dir, "coverage-id.txt")).read().strip()
# Independently reproduce the versioned, length-framed inventory identity.
def coverage(paths):
    paths = sorted(set(paths))
    h = hashlib.sha256(b"libwildcard/lake-coverage/v1\0" + struct.pack(">Q", len(paths)))
    for path in paths:
        raw = path.encode("utf-8")
        h.update(struct.pack(">Q", len(raw)))
        h.update(raw)
    return "wc-lake-v1:" + h.hexdigest()
known_rows = list(csv.reader(io.StringIO(known_csv)))
known_paths = [row[1] for row in known_rows]
assert coverage(known_paths) == coverage_id
assert all(row[0] == coverage_id for row in known_rows)
# Unrelated remote generation rows must not affect the chosen generation.
def inventory_setup(session):
    session.query(known_schema)
    session.query(known_insert + "\n" + known_csv)
    session.query("INSERT INTO wc_lake_known SELECT 'other-generation', concat('unrelated/',toString(number)) FROM numbers(9990)")
inventory_setup(s)
print("server inventory exported paths:", len(known_rows))
if len(sys.argv) > 2:
    assert len(known_rows) == 10_000
server_wp = None

cases = [("wildcard", "winpath", "*/windows/system32/*.exe"), ("like", "winpath", "//server/share/%"),
         ("wildcard", "decoded", "*/wp-admin/*"), ("like", "revdomain", "com.evil.%"),
         ("regex", "raw", r"/%2e%2e/"), ("iregex", "raw", r"login\.evil"), ("wildcard", "raw", "*/static/app.js*")]
corrupt_target = None
finite_patterns = ["ab(cd|ef)gh", "a[bc]d", "run[0-3][.]exe", "^(ab|xy)[0-9]z$"]
cases += [("regex", "raw", pat) for pat in finite_patterns]
for kind, view, pat in cases:
    def gen(*extra):
        q = subprocess.run([wc_lake, "sql", server_idx if "--known-table" in extra else idx, "--table", "T", "--select", "id", "--kind", kind, "--view", view, "--pattern", pat, "--large-inventory", "on", *extra],
                           check=True, capture_output=True, text=True).stdout.strip()
        return q.replace("FROM T", f"FROM icebergLocal('{table_dir}')")
    pruned, full = gen(), gen("--prune", "off")
    a, b = sorted(rows(pruned)), sorted(rows(full))
    cand = subprocess.run([wc_lake, "prune", idx, "--kind", kind, "--view", view, "--pattern", pat],
                          check=True, capture_output=True, text=True).stdout.splitlines()
    n_files = len(cand)
    assert a == b, (kind, pat, a, b)
    if pat in finite_patterns:
        assert len(a) == 2 and n_files == 2, (pat, a, n_files)
    print(f"{kind:8} {view:9} {pat!r:20} rows={len(a):4} indexed candidates={n_files}/10  identical to full scan")
    if pat == "*/wp-admin/*":
        corrupt_target = next(c for c in ch_paths if c not in cand)
        pruned_wp, full_wp = pruned, full
        server_wp = gen("--known-table", "wc_lake_known")
        assert "SELECT path FROM wc_lake_known" in server_wp, server_wp
        assert sorted(rows(server_wp)) == b
        assert len(server_wp) < len(pruned_wp)

# Append after indexing: both an empty old candidate set and a nonempty one must
# retain the new immutable file. The full scan is the independent result oracle.
tbl.append(pa.table({"id": [rid, rid + 1], "url": ["https://evil.test/new", "https://new.test/wp-admin/new.php"]}, schema=schema))
s = session.Session()
inventory_setup(s)
new_paths = [r[0] for r in rows(f"SELECT DISTINCT _path FROM icebergLocal('{table_dir}')") if r[0] not in ch_paths]
assert len(new_paths) == 1
new_coverage = coverage(known_paths + new_paths)
assert new_coverage != coverage_id
s.query(f"INSERT INTO wc_lake_known SELECT '{new_coverage}', path FROM wc_lake_known WHERE index_name = '{coverage_id}'")
s.query("INSERT INTO wc_lake_known VALUES ('" + new_coverage + "', '" + new_paths[0].replace("'", "\\'") + "')")
for pattern, new_id in [("*evil.test*", rid), ("*/wp-admin/*", rid + 1)]:
    def late_sql(*extra):
        generated = subprocess.run([wc_lake, "sql", server_idx if "--known-table" in extra else idx, "--table", "T", "--select", "id", "--kind", "wildcard", "--view", "decoded", "--pattern", pattern, "--large-inventory", "on", *extra],
                                   check=True, capture_output=True, text=True).stdout.strip()
        assert generated, "An old index cannot prove the current snapshot has no matches"
        return generated.replace("FROM T", f"FROM icebergLocal('{table_dir}')")
    indexed_rows, full_rows = sorted(rows(late_sql())), sorted(rows(late_sql("--prune", "off")))
    assert indexed_rows == full_rows and [new_id] in indexed_rows, (pattern, indexed_rows, full_rows)
    server_sql = late_sql("--known-table", "wc_lake_known")
    assert "SELECT path FROM wc_lake_known" in server_sql, server_sql
    assert sorted(rows(server_sql)) == full_rows
    # Missing/partial current-generation loads are safe and widen the scan.
    s.query("TRUNCATE TABLE wc_lake_known")
    assert sorted(rows(server_sql)) == full_rows
    s.query(known_insert + "\n" + known_csv.splitlines()[0] + "\n")
    assert sorted(rows(server_sql)) == full_rows
    s.query("TRUNCATE TABLE wc_lake_known")
    inventory_setup(s)
    print("append-after-index:", pattern, "preserves new-file match with complete, missing and partial server inventories")

# 3. prove skipping: corrupt a non-candidate data file
victim = next(l for l, c in lake_by_local.items() if c == corrupt_target)
with open(victim, "r+b") as f:  # destroy the Parquet footer (metadata lives at the end)
    f.truncate(os.path.getsize(victim) // 2)
# Sessions can share process-wide file caches. Start independent engines so
# neither query can reuse the intact Parquet footer read before corruption.
def fresh_query(sql, inventory=False):
    return subprocess.run(
        [sys.executable, "-c",
         "import sys,json; from chdb import session; s=session.Session(); "
         "setup,sql=json.load(sys.stdin); "
         "[s.query(q) for q in setup]; print(s.query(sql, 'JSONCompact').data())"],
        input=json.dumps(([known_schema, known_insert + "\n" + known_csv] if inventory else [], sql)),
        capture_output=True, text=True, timeout=120,
    )

pruned_result, full_result = fresh_query(pruned_wp), fresh_query(full_wp)
ok_pruned, ok_full = pruned_result.returncode == 0, full_result.returncode == 0
if ok_pruned:
    assert sorted(json.loads(pruned_result.stdout)["data"]) == sorted(rows(pruned_wp))
print(f"corrupted a non-candidate file: pruned query {'succeeded' if ok_pruned else 'FAILED'}, full query {'succeeded' if ok_full else 'failed'}")
assert ok_pruned and not ok_full, ("ClickHouse must skip non-candidate files",
                                  pruned_result.stderr, full_result.stderr)
assert "Parquet" in full_result.stderr, full_result.stderr
server_result = fresh_query(server_wp, inventory=True)
assert server_result.returncode == 0, server_result.stderr
assert sorted(json.loads(server_result.stdout)["data"]) == sorted(json.loads(pruned_result.stdout)["data"])
print("server inventory: corrupt noncandidate skipped in a fresh engine; exact rows agree")
shutil.rmtree(root)
print("RESULT: PASS")
