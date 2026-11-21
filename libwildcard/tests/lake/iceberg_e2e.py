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
import json, os, random, shutil, subprocess, sys, tempfile
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

cases = [("wildcard", "winpath", "*/windows/system32/*.exe"), ("like", "winpath", "//server/share/%"),
         ("wildcard", "decoded", "*/wp-admin/*"), ("like", "revdomain", "com.evil.%"),
         ("regex", "raw", r"/%2e%2e/"), ("iregex", "raw", r"login\.evil"), ("wildcard", "raw", "*/static/app.js*")]
corrupt_target = None
finite_patterns = ["ab(cd|ef)gh", "a[bc]d", "run[0-3][.]exe", "^(ab|xy)[0-9]z$"]
cases += [("regex", "raw", pat) for pat in finite_patterns]
for kind, view, pat in cases:
    def gen(*extra):
        q = subprocess.run([wc_lake, "sql", idx, "--table", "T", "--select", "id", "--kind", kind, "--view", view, "--pattern", pat, *extra],
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

# Append after indexing: both an empty old candidate set and a nonempty one must
# retain the new immutable file. The full scan is the independent result oracle.
tbl.append(pa.table({"id": [rid, rid + 1], "url": ["https://evil.test/new", "https://new.test/wp-admin/new.php"]}, schema=schema))
s = session.Session()
for pattern, new_id in [("*evil.test*", rid), ("*/wp-admin/*", rid + 1)]:
    def late_sql(*extra):
        generated = subprocess.run([wc_lake, "sql", idx, "--table", "T", "--select", "id", "--kind", "wildcard", "--view", "decoded", "--pattern", pattern, *extra],
                                   check=True, capture_output=True, text=True).stdout.strip()
        assert generated, "An old index cannot prove the current snapshot has no matches"
        return generated.replace("FROM T", f"FROM icebergLocal('{table_dir}')")
    indexed_rows, full_rows = sorted(rows(late_sql())), sorted(rows(late_sql("--prune", "off")))
    assert indexed_rows == full_rows and [new_id] in indexed_rows, (pattern, indexed_rows, full_rows)
    print("append-after-index:", pattern, "preserves new-file match and equals full scan")

# 3. prove skipping: corrupt a non-candidate data file
victim = next(l for l, c in lake_by_local.items() if c == corrupt_target)
with open(victim, "r+b") as f:  # destroy the Parquet footer (metadata lives at the end)
    f.truncate(os.path.getsize(victim) // 2)
# Sessions can share process-wide file caches. Start independent engines so
# neither query can reuse the intact Parquet footer read before corruption.
def fresh_query(sql):
    return subprocess.run(
        [sys.executable, "-c",
         "import sys; from chdb import session; "
         "print(session.Session().query(sys.stdin.read(), 'JSONCompact').data())"],
        input=sql, capture_output=True, text=True, timeout=120,
    )

pruned_result, full_result = fresh_query(pruned_wp), fresh_query(full_wp)
ok_pruned, ok_full = pruned_result.returncode == 0, full_result.returncode == 0
if ok_pruned:
    assert sorted(json.loads(pruned_result.stdout)["data"]) == sorted(rows(pruned_wp))
print(f"corrupted a non-candidate file: pruned query {'succeeded' if ok_pruned else 'FAILED'}, full query {'succeeded' if ok_full else 'failed'}")
assert ok_pruned and not ok_full, ("ClickHouse must skip non-candidate files",
                                  pruned_result.stderr, full_result.stderr)
assert "Parquet" in full_result.stderr, full_result.stderr
shutil.rmtree(root)
print("RESULT: PASS")
