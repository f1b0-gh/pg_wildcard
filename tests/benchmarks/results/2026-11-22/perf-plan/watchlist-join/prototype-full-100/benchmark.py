"""Private paired MV spike; no Elastic timing equivalence is implied."""
import argparse
import hashlib
import json
import os
import re
import signal
import statistics
import sys
import time
import uuid
from pathlib import Path

sys.path.insert(0, 'tests/benchmarks')
from compare_routes import sweeps, native_setup
from comparison import expected_pairs, require_wildcard_target, schedule, verify_pairs
from prototype import render

p = argparse.ArgumentParser()
p.add_argument('--rows', type=int, choices=(10000, 1000000), default=10000)
p.add_argument('--repeats', type=int, choices=(1, 5), default=1)
p.add_argument('--indicators', type=int, choices=(100, 1000), default=100)
p.add_argument('--families', default='domains,paths,urls,commands')
p.add_argument('--spec-dir', type=Path, default=Path('.local/perf/anchored-needles/specs'))
p.add_argument('--out', type=Path, required=True)
a = p.parse_args()
families = a.families.split(',')
if len(set(families)) != len(families) or set(families) - {'domains', 'paths', 'urls', 'commands', 'shared', 'unrelated'}:
    raise ValueError('unsupported or repeated fixture family')
a.out.mkdir(exist_ok=False)
(a.out / 'provenance.json').write_text(json.dumps({name: hashlib.sha256(
    (Path(__file__).parent / name).read_bytes()).hexdigest()
    for name in ('benchmark.py', 'prototype.py', 'qualify.py')}, indent=2) + '\n')
for name in ('benchmark.py', 'prototype.py', 'qualify.py'):
    (a.out / name).write_bytes((Path(__file__).parent / name).read_bytes())
marker = Path('.local/perf/execution/timing-active')


def terminate(_signum, _frame):
    raise KeyboardInterrupt('owned percolator benchmark termination')


signal.signal(signal.SIGTERM, terminate)
for family in families:
    if marker.exists():
        raise RuntimeError('another timing window is active')
    marker.write_text('percolator MV ' + family + '\n')
    engine = None
    out = a.out / family
    out.mkdir()
    try:
        spec = json.loads((a.spec_dir / f'{family}-{a.indicators}-1000000.json').read_text())
        require_wildcard_target(spec)
        if len(spec['patterns']) != a.indicators:
            raise ValueError('indicator count differs from requested experiment')
        spec['rows'] = a.rows
        candidate, diagnostics = render(spec)
        if candidate is None:
            raise ValueError(diagnostics)
        variants = {'staged_needles': spec['watchlist']['mv'], 'rule_join': candidate}
        view_names = {}
        for name, sql in variants.items():
            match = re.match(r'CREATE MATERIALIZED VIEW (?:IF NOT EXISTS )?(`[^`]+`|[A-Za-z_]\w*) ', sql)
            if match is None:
                raise ValueError('unexpected generated MV prefix')
            view_names[name] = match[1]
        (out / 'spec.json').write_text(json.dumps(spec, sort_keys=True) + '\n')
        (out / 'variants.json').write_text(json.dumps(variants, indent=2) + '\n')
        (out / 'diagnostics.json').write_text(json.dumps(diagnostics, indent=2) + '\n')
        engine = sweeps.Engine('.local/perf/antalya.json', timeout_seconds=300)
        native_setup(engine, spec, out, False)
        engine.query(spec['watchlist']['ddl'])
        sweeps.inserts(engine, spec['watchlist']['inserts'])
        engine.query('CREATE TABLE ingest_src(id Int64,v String) ENGINE=MergeTree ORDER BY id')
        engine.query('CREATE TABLE ingest_hits(id Int64,indicator_id Int64,view String) ENGINE=MergeTree ORDER BY id')

        def install(label):
            for name in view_names.values():
                engine.query('DROP VIEW IF EXISTS ' + name)
            engine.query(variants[label])
            engine.query('TRUNCATE TABLE ingest_src')
            engine.query('TRUNCATE TABLE ingest_hits')

        def pairs():
            return [tuple(row) for row in engine.query('SELECT id,indicator_id,view FROM ingest_hits')]

        canary = [(row, int(hit[0]), str(hit[1])) for row, hits in enumerate(spec['template_hits']) for hit in hits]
        for label in variants:
            install(label)
            engine.query('INSERT INTO ingest_src SELECT toInt64(id),v FROM templates')
            verify_pairs(pairs(), canary)
        (out / 'settings.json').write_text(json.dumps(dict(
            engine=engine.query('SELECT version()')[0][0], settings=engine.settings,
            machine=sweeps.machine_settings(), rows=a.rows, indicators=a.indicators,
            repeats=a.repeats, timer='native resident-source INSERT SELECT through actual MV persistence; validation after timer',
            baseline='current staged three-needle metadata, including anchored-interior correction'), indent=2) + '\n')
        expected = expected_pairs(spec)
        records = []
        for warmup, repeat, label in schedule(list(variants), a.repeats):
            install(label)
            qid = engine.prefix + uuid.uuid4().hex
            pending = dict(strategy=label, warmup=warmup, repeat=repeat, query_id=qid)
            (out / 'pending.json').write_text(json.dumps(pending) + '\n')
            started = time.monotonic()
            engine.query('INSERT INTO ingest_src SELECT id,v FROM bench', query_id=qid)
            elapsed = (time.monotonic() - started) * 1000
            digest = verify_pairs(pairs(), expected)
            record = dict(**pending, request_ms=elapsed, pairs=len(expected), pair_sha256=digest,
                          profile=engine.profile(qid), host_load=list(os.getloadavg()))
            with (out / 'raw.jsonl').open('a') as f:
                f.write(json.dumps(record) + '\n')
            records.append(record)
            (out / 'pending.json').unlink()
            print(family, label, warmup, repeat, round(elapsed, 2), flush=True)
        summary = {}
        for label in variants:
            values = [r['request_ms'] for r in records if r['strategy'] == label and not r['warmup']]
            summary[label] = dict(samples=len(values), median_ms=statistics.median(values),
                                  min_ms=min(values), max_ms=max(values))
        (out / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    finally:
        try:
            if engine:
                engine.close()
        finally:
            marker.unlink(missing_ok=True)
