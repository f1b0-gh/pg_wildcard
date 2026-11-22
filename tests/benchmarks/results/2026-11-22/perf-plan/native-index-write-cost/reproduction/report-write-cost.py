"""Validate and summarize the completed paired byte-index write experiment."""
import hashlib, json, re, statistics
from pathlib import Path

root = Path('.local/perf/indexed/write-cost')
records = [json.loads(line) for line in (root / 'raw.jsonl').read_text().splitlines()]
trials = [r for r in records if 'strategy' in r]
assert len(trials) == 10
assert {(r['strategy'], r['repeat']) for r in trials} == {(s, i) for s in ('plain', 'byte_index') for i in range(5)}
assert len({r['corpus_sha256'] for r in trials}) == 1
assert all(r['storage']['rows'] == 1000000 for r in trials)
assert all(r['storage']['data_compressed_bytes'] == 9375381 for r in trials)
recovered = json.loads((root / 'recovered-profiles.json').read_text())
assert recovered['found_inserts'] == recovered['expected_inserts'] == 20
profiles = {}
for p in recovered['profiles']:
    m = re.fullmatch(r'INSERT INTO (plain|byte_index)_([0-4]) SELECT \* FROM bench WHERE id(<900000|>=900000)', p['query'])
    assert m, p['query']
    strategy, repeat, predicate = m.groups()
    stage = 'initial_900k' if predicate == '<900000' else 'incremental_100k'
    key = strategy, int(repeat), stage
    assert key not in profiles
    assert p['written_rows'] == (900000 if stage == 'initial_900k' else 100000)
    profiles[key] = p
assert len({p['query_id'] for p in profiles.values()}) == 20
stats = lambda xs: dict(median=statistics.median(xs), min=min(xs), max=max(xs))
summary = json.loads((root / 'summary.json').read_text())
output = []
for strategy in ('plain', 'byte_index'):
    rows = sorted((r for r in trials if r['strategy'] == strategy), key=lambda r: r['repeat'])
    for stage in ('initial_900k', 'incremental_100k'):
        clock = stats([r[stage + '_seconds'] for r in rows])
        assert summary[strategy][stage + '_seconds'] == clock
        ps = [profiles[strategy, r['repeat'], stage] for r in rows]
        output.append(dict(strategy=strategy, stage=stage, repetitions=5,
            request_seconds=clock,
            server_ms=stats([p['query_duration_ms'] for p in ps]),
            native_cpu_ms=stats([(p['ProfileEvents']['UserTimeMicroseconds'] + p['ProfileEvents']['SystemTimeMicroseconds']) / 1000 for p in ps]),
            query_peak_mib=stats([p['memory_usage'] / (1 << 20) for p in ps]),
            read_rows=stats([p['read_rows'] for p in ps])))
storage = {s: {k: stats([r['storage'][k] for r in trials if r['strategy'] == s]) for k in ('total_bytes', 'data_compressed_bytes', 'index_compressed_bytes')} for s in ('plain', 'byte_index')}
fmt = lambda x: f"{x['median']:.3f} [{x['min']:.3f}, {x['max']:.3f}]"
lines = ['# Bounded byte-index write and storage cost', '',
    'Five paired, alternating-order trials on fresh tables. Each table receives '
    '900,000 initial rows, then 100,000 incremental rows. Native HTTP request clocks '
    'include INSERT execution and completion; server durations are separate diagnostics. '
    'The text index is maintained during insertion, so these are write costs, '
    'not timings of a separate backfill operation. No antalya-rs, PostgreSQL or Elastic '
    'route is measured here.', '',
    '| Layout | Insert rows | Request seconds median [range] | Native server ms median [range] | Native CPU ms median [range] | Query peak MiB median [range] |',
    '|---|---:|---:|---:|---:|---:|']
for r in output:
    lines.append(f"| {r['strategy']} | {900000 if r['stage']=='initial_900k' else 100000} | {fmt(r['request_seconds'])} | {fmt(r['server_ms'])} | {fmt(r['native_cpu_ms'])} | {fmt(r['query_peak_mib'])} |")
lines += ['', 'All ten full one-million-row corpus hashes agree. Tables were settled '
    'with OPTIMIZE FINAL before storage inspection. Source-data compressed bytes '
    'are identical, which makes the additional compressed index bytes explicit.', '',
    '| Layout | Total bytes median | Source compressed bytes median | Index compressed bytes median |',
    '|---|---:|---:|---:|']
for s, values in storage.items():
    lines.append('| ' + s + ' | ' + ' | '.join(f"{values[k]['median']:,}" for k in ('total_bytes','data_compressed_bytes','index_compressed_bytes')) + ' |')
base = next(r for r in output if r['strategy']=='plain' and r['stage']=='incremental_100k')['request_seconds']['median']
indexed = next(r for r in output if r['strategy']=='byte_index' and r['stage']=='incremental_100k')['request_seconds']['median']
lines += ['', f'The incremental insertion median is {indexed/base:.2f}× the plain layout '
    'in this fixture. This is a material tradeoff even where the separate query '
    'experiments improve latency. It does not establish useful file/mark pruning '
    'or justify enabling the prototype for every field.', '',
    'The bounded expression indexes raw three-byte substrings, retaining an overflow '
    'sentinel for values beyond its cap. Exact DDL and per-trial host load are in '
    'raw.jsonl. This private prototype remains separate from the production text-index '
    'layout and the opt-in native watchlist join.', '',
    'Provenance limitation: the original private runner did not record a dedicated '
    'start-of-run machine/settings snapshot or query IDs. All twenty original '
    'INSERT QueryFinish records, including their IDs, settings and counters, were '
    'recovered from the owned server after the host reboot; no timings were replayed. '
    'The recovery file explicitly labels its machine and source-hash observations '
    'as post-run. Query-log settings are the engine-recorded map, not a complete '
    'snapshot of every default. A later host snapshot cannot prove the earlier '
    'configuration. The original raw clocks and summary are preserved.', '',
    'Reproduce the report (no engine calls): '
    '`python3 .local/perf/indexed/report-write-cost.py`.']
(root / 'report.md').write_text('\n'.join(lines) + '\n')
(root / 'report.json').write_text(json.dumps(dict(cases=output, storage=storage,
    corpus_sha256=trials[0]['corpus_sha256'], query_profiles_recovered=20,
    sources={p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in [root/'raw.jsonl',root/'summary.json',root/'recovered-profiles.json']},
    provenance='original timings; post-run recovery of exact INSERT profiles; machine snapshot unavailable at start'), indent=2) + '\n')
print(root / 'report.md')
