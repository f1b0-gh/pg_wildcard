"""Bounded unfavorable-workload controls; native exact multisets are authoritative."""
import hashlib
import json
import os
import signal
import sys
import time
import uuid
from pathlib import Path
sys.path.insert(0, 'tests/benchmarks')
from compare_routes import sweeps
from comparison import schedule, verify_pairs
from prototype import render

root = Path('.local/perf/percolator/fallback-controls')
marker = Path('.local/perf/execution/timing-active')
if marker.exists():
    raise RuntimeError('another measurement is active')
root.mkdir(exist_ok=False)
sources = [Path(__file__), Path(__file__).with_name('prototype.py'),
           Path('libwildcard/tests/benchmarks/sweeps.py'),
           Path('tests/benchmarks/comparison.py')]
provenance = {}
for source in sources:
    contents = source.read_bytes()
    (root / source.name).write_bytes(contents)
    provenance[str(source)] = hashlib.sha256(contents).hexdigest()
(root / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
spec = json.loads(Path('.local/perf/percolator/specs/shared-1000-1000000.json').read_text())


def terminate(_signum, _frame):
    raise KeyboardInterrupt('owned fallback probe termination')


signal.signal(signal.SIGTERM, terminate)
summary = []
for case in ('no_grams', 'common_gram', 'stale_bindings', 'long_values'):
    marker.write_text('percolator fallback ' + case + '\n')
    out = root / case
    out.mkdir()
    engine = None
    try:
        engine = sweeps.Engine('.local/perf/antalya.json', timeout_seconds=120)
        engine.query(spec['watchlist']['ddl'])
        engine.query('CREATE TABLE ingest_src(id Int64,v String) ENGINE=MergeTree ORDER BY id')
        engine.query('CREATE TABLE ingest_hits(id Int64,indicator_id Int64,view String) ENGINE=MergeTree ORDER BY id')
        engine.query('CREATE TABLE bench(id Int64,v String) ENGINE=MergeTree ORDER BY id')
        if case == 'no_grams':
            regex, needles, binding = 'a|xNNNN', [], 'same as regex'
            positive, negative = 'a', 'bbbbbbb'
        elif case == 'common_gram':
            regex, needles, binding = r'tag[0-9]{4}', ['tag'], r'tag[0-9]{4}'
            positive, negative = 'tag0000', 'tagzzzz'
        elif case == 'stale_bindings':
            regex, needles, binding = 'tag0000', ['zzz'], 'zzz'
            positive, negative = 'tag0000', 'nomatch'
        else:
            regex, needles, binding = 'tagNNNN', ['tag'], 'same as regex'
            positive, negative = 'x' * 512 + 'tag0000', 'x' * 512
        q = sweeps.literal
        regex_expr = ("concat('a|x',leftPad(toString(number),4,'0'))" if case == 'no_grams'
                      else "concat('tag',leftPad(toString(number),4,'0'))" if case in ('stale_bindings','long_values')
                      else q(regex))
        binding_expr = q(binding) if case == 'stale_bindings' else regex_expr
        needle_sql = '[' + ','.join(map(q, needles)) + ']'
        engine.query('INSERT INTO watch (id,view,regex,needles,needles_regex,needles_view) '
                     + f"SELECT toInt64(number),'raw',{regex_expr},{needle_sql},{binding_expr},'raw' FROM numbers(1000)")
        engine.query('INSERT INTO bench SELECT toInt64(number),'
                     + f'if(number=0,{q(positive)},{q(negative)}) FROM numbers(10000)')
        candidate, diagnostics = render({'patterns': [dict(view='raw')]})
        variants = {'staged_needles': spec['watchlist']['mv'], 'rule_join': candidate}
        (out / 'settings.json').write_text(json.dumps(dict(
            engine=engine.query('SELECT version()')[0][0], settings=engine.settings,
            machine=sweeps.machine_settings(), rows=10000, indicators=1000,
            regex=regex, regex_expression=regex_expr, needles=needles, binding_expression=binding_expr,
            positive=positive, negative=negative,
            candidate_pairs=10000 * 1000,
            candidate_pair_meaning='logical join candidates: every row reaches all rules in these unfavorable cases; not measured regex calls',
            repeats=5, timer='native resident-source INSERT SELECT through source/hit persistence; validation after timer',
            scope='bounded unfavorable cases, not a million-row scaling claim',
            engine_resources={key: engine.state[key] for key in ('cpus', 'memory_bytes')},
            engine_image=engine.state.get('image'), diagnostics=diagnostics), indent=2) + '\n')
        (out / 'variants.json').write_text(json.dumps(variants, indent=2) + '\n')
        (out / 'watchlist.sql').write_text(spec['watchlist']['ddl'] + '\n')
        expected = [tuple(r) for r in engine.query("SELECT s.id,p.id,p.view FROM bench AS s CROSS JOIN "
            "(SELECT id,regex,view FROM watch FINAL WHERE active=1 AND view='raw') AS p WHERE match(s.v,p.regex)")]
        expected_count = 1000 if case in ('no_grams', 'common_gram') else 1
        if len(expected) != expected_count:
            raise AssertionError('positive-row exact oracle differs from the fixture contract')
        failed = set()
        for warmup, repeat, label in schedule(list(variants), 5):
            if label in failed:
                continue
            engine.query('DROP VIEW IF EXISTS ingest_mv')
            engine.query('DROP VIEW IF EXISTS percolator_mv')
            engine.query('TRUNCATE TABLE ingest_src')
            engine.query('TRUNCATE TABLE ingest_hits')
            engine.query(variants[label])
            qid = engine.prefix + uuid.uuid4().hex
            pending = dict(case=case, strategy=label, warmup=warmup, repeat=repeat, query_id=qid)
            (out / 'pending.json').write_text(json.dumps(pending) + '\n')
            started = time.monotonic()
            try:
                engine.query('INSERT INTO ingest_src SELECT id,v FROM bench', query_id=qid)
            except Exception as error:
                record = dict(**pending, status='failed_insertion', elapsed_ms=(time.monotonic()-started)*1000,
                              error=sweeps.redact(str(error), engine.state), no_latency_sample=True)
                (out / 'pending.json').write_text(json.dumps(record) + '\n')
                failed.add(label)
                # A client timeout alone does not establish a terminal server query.
                # Cancel only this insertion and prove it is gone before another timer.
                engine.query('KILL QUERY WHERE query_id=' + q(qid) + ' SYNC', logging=False)
                live = engine.query('SELECT count() FROM system.processes WHERE query_id=' + q(qid), logging=False)
                if live != [[0]]:
                    raise RuntimeError('failed insertion is still live; do not run another timing')
                record['server_query_absent_after_cancel'] = True
                engine.query('SYSTEM FLUSH LOGS', logging=False)
                record['terminal_log'] = engine.query(
                    'SELECT toString(type),exception_code,query_duration_ms,read_rows,memory_usage '
                    'FROM system.query_log WHERE query_id=' + q(qid)
                    + " AND type IN ('QueryFinish','ExceptionBeforeStart','ExceptionWhileProcessing')"
                    + ' ORDER BY event_time_microseconds DESC LIMIT 1', logging=False)
            else:
                elapsed = (time.monotonic()-started)*1000
                actual = [tuple(r) for r in engine.query('SELECT id,indicator_id,view FROM ingest_hits')]
                digest = verify_pairs(actual, expected)
                record = dict(**pending, status='complete', request_ms=elapsed, pairs=len(actual),
                              pair_sha256=digest, profile=engine.profile(qid))
            record['host_load'] = list(os.getloadavg())
            with (out / 'raw.jsonl').open('a') as f:
                f.write(json.dumps(record) + '\n')
            (out / 'pending.json').unlink()
            print(case, label, warmup, repeat, record['status'], round(record.get('request_ms',record.get('elapsed_ms')),2), flush=True)
        summary.append(dict(case=case, completed_strategies=sorted(set(variants)-failed), failed_strategies=sorted(failed)))
        (root / 'status.json').write_text(json.dumps(summary, indent=2) + '\n')
    finally:
        try:
            if engine:
                engine.close()
            (out / 'cleanup.json').write_text(json.dumps({'status': 'complete'}) + '\n')
        except Exception as error:
            (out / 'cleanup.json').write_text(json.dumps({
                'status': 'failed', 'error': sweeps.redact(str(error), engine.state if engine else None)}) + '\n')
            raise
        finally:
            marker.unlink(missing_ok=True)
