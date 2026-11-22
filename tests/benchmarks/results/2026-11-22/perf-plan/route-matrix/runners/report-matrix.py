"""Summarize only completed route cases, validating raw samples and exact oracles."""
import json, statistics, sys
from pathlib import Path
sys.path.insert(0, 'tests/benchmarks')
from comparison import expected_pairs, pair_digest
from compare_routes import foreign_nodes

root = Path('.local/perf')
status = json.loads((root / 'execution/matrix-status.json').read_text())
cases = []
omissions = []
storage = []
fixtures = []
stats = lambda xs: dict(median=statistics.median(xs), min=min(xs), max=max(xs)) if xs else None
for entry in status:
    name = entry['case']
    if entry['status'] != 'complete':
        omissions.append(entry)
        continue
    directory = root / 'matrix' / name
    spec = json.loads((directory / 'spec.json').read_text())
    settings = json.loads((directory / 'settings.json').read_text())
    summaries = json.loads((directory / 'summary.json').read_text())
    raw = [json.loads(line) for line in (directory / 'raw.jsonl').read_text().splitlines()]
    assert not any(r.get('kind') == 'cleanup_failure' for r in raw)
    assert not (directory / 'pending.json').exists()
    expected = expected_pairs(spec)
    digest = pair_digest(expected)
    unique_patterns=len({(p['regex'],p['view']) for p in spec['patterns']})
    fixtures.append(dict(case=name,rows=spec['rows'],indicator_entries=len(spec['patterns']),
        unique_regex_view_pairs=unique_patterns,template_values=len(spec['templates']),
        expected_output_pairs=len(expected),elastic_supported=spec['elastic_supported'],
        duplicate_pattern_entries=len(spec['patterns'])-unique_patterns))
    concurrency = settings['concurrency']
    assert settings['repeats'] == 5
    for row in raw:
        if row.get('kind') in ('excluded', 'index_unavailable', 'precontrol_failure'):
            omissions.append(dict(case=name, **row))
        if row.get('kind') in ('source', 'elastic_source', 'index_build'):
            if row.get('kind') != 'index_build':
                assert row['corpus_sha256'] == settings['corpus_sha256']
            storage.append(dict(case=name, **row))
    for summary in summaries:
        route = summary['route']
        cohorts = [r for r in raw if r.get('kind') == 'cohort' and r['route'] == route]
        precontrols=[r for r in raw if r.get('kind')=='precontrol_failure' and route in r['routes']]
        if precontrols:
            assert len(precontrols)==1 and precontrols[0]['cleanup_confirmed'] and precontrols[0]['timed'] is False
            assert route in ('pg_backend','pg_copy') and summary['censored'] and not cohorts
        successes = []
        failures = []
        measured = []
        for cohort in cohorts:
            assert cohort['requested_concurrency'] == concurrency
            assert len(cohort['results']) == concurrency
            assert {r['worker'] for r in cohort['results']} == set(range(concurrency))
            failed = False
            for r in cohort['results']:
                if 'error' in r:
                    assert not r.get('correctness_failure'), 'an oracle failure is never a censored performance result'
                    assert cohort.get('cleanup_confirmed')
                    failures.append(dict(warmup=cohort['warmup'], repeat=cohort['repeat'], error=r['error']))
                    failed = True
                    continue
                if route == 'pg_backend':
                    nodes = list(foreign_nodes(r['plan']['Plan']))
                    assert sum(n['Actual Rows'] * n['Actual Loops'] for n in nodes) == len(expected)
                    assert r['oracle'].startswith('identical SQL full COPY validated')
                else:
                    assert r['pairs'] == len(expected) and r['pair_sha256'] == digest
                if not cohort['warmup']:
                    successes.append(r)
            if not cohort['warmup'] and not failed:
                measured.append(cohort)
        values = [r['request_ms'] for r in successes]
        observed = stats(values)
        assert summary['samples'] == len(values)
        assert summary['median_ms'] == (observed['median'] if observed else None)
        assert summary['min_ms'] == (observed['min'] if observed else None)
        assert summary['max_ms'] == (observed['max'] if observed else None)
        if not summary['censored']:
            assert len(measured) == 5 and {r['repeat'] for r in measured} == set(range(5))
            assert len(successes) == 5 * concurrency
            assert sum(c['warmup'] for c in cohorts) == 1
        all_profiles = []
        for c in measured:
            ps = c.get('profiles', []) + [p for r in c['results'] for p in r.get('profiles', [])]
            if ps:
                all_profiles.append(dict(read_rows=sum(p['read_rows'] for p in ps),
                    read_bytes=sum(p['read_bytes'] for p in ps),
                    cpu_ms=sum(p['profile_events'].get('UserTimeMicroseconds',0) + p['profile_events'].get('SystemTimeMicroseconds',0) for p in ps)/1000,
                    largest_query_bytes=max(p['memory_usage'] for p in ps), queries=len(ps)))
        cases.append(dict(case=name, route=route, concurrency=concurrency,
            precontrol_failures=precontrols,
            complete_cohorts=len(measured), client_samples=len(values), censored=summary['censored'],
            request_ms=observed, executor_ms=stats([r['executor_ms'] for r in successes]) if route=='pg_backend' else None,
            client_sweeps_per_second=stats([concurrency / c['cohort_seconds'] for c in measured]),
            source_query_count_per_client=stats([r['source_queries'] for r in successes if 'source_queries' in r]),
            native_total_cohort={k:stats([p[k] for p in all_profiles]) for k in ('read_rows','read_bytes','cpu_ms','queries')},
            largest_native_query_bytes=max((p['largest_query_bytes'] for p in all_profiles), default=None),
            observed_client_peak=max((c['observed_client_peak'] for c in cohorts), default=None),
            observed_native_or_elastic_search_peak=summary['observed_engine_peak'],
            sampling_errors=summary['sampler_errors'], failures=failures,
            corpus_sha256=settings['corpus_sha256'], pattern_sha256=settings['pattern_sha256'],
            expected_pairs_per_client=len(expected), pair_sha256=digest,
            indicator_entries=len(spec['patterns']), unique_regex_view_pairs=unique_patterns,
            versions=settings['versions'], engine=settings['engine']))

lines = ['# Comparable route matrix — completed cases', '',
    'Each row is a separately measured route on the same admitted wildcard fixture. '
    'Request medians/ranges use complete consumption, with correctness checks outside '
    'that clock. PostgreSQL backend request includes EXPLAIN transport; its TIMING OFF '
    'executor clock is reported separately. Native HTTP, direct published antalya-rs, '
    'PostgreSQL backend, full binary COPY and Elastic are not interchangeable timers. '
    'Keyword is a historical Elastic storage-layout control, not application field admission.', '',
    'Five barrier-started measured cohorts follow one warmup for uncensored routes. '
    'At concurrency four, twenty clients are nested in five cohorts; they are not '
    'twenty independent repetitions. Filesystem caches are uncontrolled. Engine resources, '
    'host load, limits and precise SQL/DSL remain in each case directory. Cases spanning '
    'the host reboot are separate sessions, not paired code comparisons.', '',
    '| Case | Route | Complete cohorts / client samples | Request ms median [range] | Executor ms median [range] | Observed clients / engine searches | Status |',
    '|---|---|---:|---:|---:|---:|---|']
fmt = lambda v: '—' if v is None else f"{v['median']:.2f} [{v['min']:.2f}, {v['max']:.2f}]"
for r in cases:
    lines.append(f"| {r['case']} | {r['route']} | {r['complete_cohorts']} / {r['client_samples']} | {fmt(r['request_ms'])} | {fmt(r['executor_ms'])} | {r['observed_client_peak']} / {r['observed_native_or_elastic_search_peak']} | {'censored; partial observations only' if r['censored'] else 'complete'} |")
lines += ['', 'Censored rows preserve any completed observations but do not establish '
    'a five-repetition speed ratio. Unsupported semantic subsets and bounded stress '
    'refusals are listed below. A backend-only sample checks executor rows against '
    'SQL whose full COPY pairs were validated before measurement; it does not deliver '
    'those pairs to the client during its clock.', '',
    'The JSON retains query-log reads/CPU for all clients in each cohort, not an '
    'assumed scan count. Query count is separate from rows/marks read. Engine peak '
    'is a sampled active-search count, not verified disk parallelism. Elastic '
    'uses one inner search at a time per msearch; requested client concurrency is '
    'a separate setting. Client-sweep throughput in JSON uses the external cohort '
    'makespan, including protocol handling and any worker-side decoding outside '
    'the inner request timer. It must not be described as isolated engine throughput.', '',
    '## Fixture diversity and output fanout', '',
    'Indicator count is not necessarily distinct pattern count. The short/no-gram '
    'fixture deliberately repeats one exact single-character pattern, `q`, under '
    'different IDs. It tests empty-gram fallback and preservation of every matching '
    'ID; it does not represent a diverse short-wildcard watchlist. Do not extrapolate '
    'its route ordering to general short patterns or regex engines. Exact output '
    'pairs, source selectivity and pattern diversity are separate dimensions.', '',
    '| Case | Rows | Indicator entries | Unique regex/view pairs | Template values | Expected output pairs | Elastic shared subset |',
    '|---|---:|---:|---:|---:|---:|---|']
for f in fixtures:
    lines.append(f"| {f['case']} | {f['rows']} | {f['indicator_entries']} | {f['unique_regex_view_pairs']} | {f['template_values']} | {f['expected_output_pairs']} | {f['elastic_supported']} |")
lines += ['',
    '## Explicit omissions and bounded outcomes', '']
lines.extend('- ' + json.dumps(r, sort_keys=True) for r in omissions)
if not omissions: lines.append('None among completed cases; the remaining planned cases are still pending.')
lines += ['', 'This report covers only cases marked complete in matrix-status.json. '
    'It does not infer completion of the full matrix. Build/insert/storage observations '
    'are retained separately in matrix-storage.json, with per-case layout labels.']
(root / 'matrix-report.md').write_text('\n'.join(lines)+'\n')
(root / 'matrix-report.json').write_text(json.dumps(dict(cases=cases, fixtures=fixtures, omissions=omissions, completed_cases=[x['case'] for x in status if x['status']=='complete']),indent=2)+'\n')
(root / 'matrix-storage.json').write_text(json.dumps(storage,indent=2)+'\n')
print('Validated', len(cases), 'route rows in', sum(x['status']=='complete' for x in status), 'completed cases')
