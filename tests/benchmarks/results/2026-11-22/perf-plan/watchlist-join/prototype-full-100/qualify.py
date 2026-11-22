"""Bounded native correctness spike; never a performance result."""
import json
import signal
import sys
from pathlib import Path

sys.path.insert(0, 'tests/benchmarks')
from compare_routes import sweeps
from comparison import verify_pairs
from prototype import render

root = Path('.local/perf/percolator')
out = root / 'qualification'
marker = Path('.local/perf/execution/timing-active')
if marker.exists():
    raise RuntimeError('another timing window is active')
out.mkdir(exist_ok=False)
marker.write_text('percolator native correctness probe; untimed\n')


def terminate(_signum, _frame):
    raise KeyboardInterrupt('owned percolator probe termination')


signal.signal(signal.SIGTERM, terminate)
engine = None
status = []
try:
    spec = json.loads(Path('.local/perf/anchored-needles/specs/paths-100-1000000.json').read_text())
    patterns = [dict(p) for p in spec['patterns']]
    extra = [('tag000', ['tag', 'ag0', '000']),
             ('tag000', ['tag', 'ag0', '000']),
             ('a|b', []), ('(?i)Ꭰ', []), (r'\x00', []), ('', []),
             (r'\Atag000\z', ['tag', 'ag0', '000'])]
    patterns.extend(dict(id=1000+i, view='raw', regex=regex, needles=needles)
                    for i, (regex, needles) in enumerate(extra))
    sql, diagnostics = render({'patterns': patterns})
    if sql is None:
        raise AssertionError('raw spike unexpectedly declined')
    if render({'patterns': [dict(view='lower')]})[0] is not None:
        raise AssertionError('unsupported view did not request fallback')
    (out / 'mv.sql').write_text(sql + '\n')
    (out / 'patterns.json').write_text(json.dumps(patterns, indent=2) + '\n')
    (out / 'diagnostics.json').write_text(json.dumps(diagnostics, indent=2) + '\n')
    engine = sweeps.Engine('.local/perf/antalya.json', timeout_seconds=30)
    engine.query(spec['watchlist']['ddl'])
    engine.query('CREATE TABLE ingest_src(id Int64,v String) ENGINE=MergeTree ORDER BY id')
    engine.query('CREATE TABLE ingest_hits(id Int64,indicator_id Int64,view String) ENGINE=MergeTree ORDER BY id')
    engine.query(sql)

    def insert_rule(p, version, *, binding=None, bound_view=None, active=1):
        q = sweeps.literal
        needles = '[' + ','.join(q(n) for n in p['needles']) + ']'
        engine.query('INSERT INTO watch (id,view,regex,needles,needles_regex,needles_view,active,updated_at) VALUES ('
                     + f"{p['id']},{q(p['view'])},{q(p['regex'])},{needles},"
                     + f"{q(p['regex'] if binding is None else binding)},"
                     + f"{q(p['view'] if bound_view is None else bound_view)},{active},"
                     + f'fromUnixTimestamp64Milli({version}))')

    for p in patterns:
        insert_rule(p, 1000)
    values = [(i, value.encode()) for i, value in enumerate(spec['templates'])]
    values += [(10000+i, value) for i, value in enumerate([
        b'', b'a', b'ba', b'\xff\0tag000', b'tag000 tag000',
        b'tag000', b'x'*256+b'tag000', b'tag000'+b'x'*100000,
        'Ꭰꭰ'.encode(), b'\xc0tag000', b'\x80', b'changed123',
        b'C:\\Users\\alice\\Temp\\drop.exe',
    ])]
    # Preserve duplicate source rows, including repeated IDs, as a multiset.
    values += [(10005, b'tag000'), (10005, b'changed123')]

    def check(phase):
        engine.query('TRUNCATE TABLE ingest_src')
        engine.query('TRUNCATE TABLE ingest_hits')
        for start in range(0, len(values), 32):
            engine.query('INSERT INTO ingest_src VALUES ' + ','.join(
                f"({row},unhex('{value.hex()}'))" for row, value in values[start:start+32]))
        expected = [tuple(r) for r in engine.query(
            "SELECT s.id,p.id,p.view FROM ingest_src AS s CROSS JOIN "
            "(SELECT id,regex,view FROM watch FINAL WHERE active=1 AND view='raw') AS p "
            'WHERE match(s.v,p.regex)')]
        actual = [tuple(r) for r in engine.query('SELECT id,indicator_id,view FROM ingest_hits')]
        digest = verify_pairs(actual, expected)
        status.append(dict(phase=phase, status='PASS', pairs=len(actual), pair_sha256=digest))
        (out / 'status.json').write_text(json.dumps(status, indent=2) + '\n')
        print(phase, 'PASS', len(actual), flush=True)

    check('initial_positive_near_miss_bytes_overflow_duplicates')
    p = dict(patterns[100], regex='changed123')
    insert_rule(p, 2000, binding='tag000')
    check('changed_regex_with_stale_needles')
    insert_rule(patterns[101], 2000, bound_view='lower')
    check('stale_view_binding')
    insert_rule(patterns[102], 2000, active=0)
    check('deactivated_rule')
    engine.query('TRUNCATE TABLE watch')
    check('empty_watchlist')
    (out / 'settings.json').write_text(json.dumps(dict(
        engine=engine.query('SELECT version()')[0][0], settings=engine.settings,
        scope='raw-view native exact-pair correctness only; no timing claim'), indent=2) + '\n')
finally:
    try:
        if engine:
            engine.close()
    finally:
        marker.unlink(missing_ok=True)
