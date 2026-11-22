"""Record the failed retention gate without attributing shared-host noise to code."""
import json
import statistics
from pathlib import Path

root = Path('.local/perf/fdw')
status = json.loads((root / 'comparison-inband-status.json').read_text())
names = {f'inband-{label}-{rep}' for label in ('baseline', 'prepared') for rep in range(5)}
if len(status) != 10 or {r['name'] for r in status} != names or any(r['exit_status'] for r in status):
    raise ValueError('ten successful binary rounds required')
workloads = ('narrow', 'smalltext', 'wide1k', 'broad_recheck')
routes = ('direct', 'pg_backend', 'pg_copy')
samples = {}
for entry in status:
    directory = root / entry['name']
    if json.loads((directory / 'cleanup.json').read_text())['failures']:
        raise ValueError('unclean benchmark')
    records = [json.loads(line) for line in (directory / 'raw.jsonl').read_text().splitlines()]
    measured = [r for r in records if r.get('kind') == 'cohort' and not r['warmup']]
    if len(measured) != 12 or {(r['workload'], r['route']) for r in measured} != {(w, r) for w in workloads for r in routes}:
        raise ValueError('missing or duplicate workload/route')
    for cohort in measured:
        if cohort['requested_concurrency'] != 1 or len(cohort['results']) != 1:
            raise ValueError('unexpected concurrency')
        result = cohort['results'][0]
        rows = 32768 if cohort['workload'] == 'wide1k' else 1000000
        route = cohort['route']
        if route == 'direct' and result['rows'] != rows:
            raise ValueError('direct row mismatch')
        if route == 'pg_backend' and result['plan']['Plan']['Actual Rows'] != rows:
            raise ValueError('backend row mismatch')
        if route == 'pg_copy' and (result['value_control']['rows'] != rows or result['value_control']['oracle'] != 'every field and complete unique row-ID set'):
            raise ValueError('COPY value control differs')
        samples[entry['name'], cohort['workload'], route] = result['request_ms']

stats = lambda xs: dict(median=statistics.median(xs), min=min(xs), max=max(xs))
results = []
for workload in workloads:
    for route in routes:
        baseline = [samples[f'inband-baseline-{i}', workload, route] for i in range(5)]
        prepared = [samples[f'inband-prepared-{i}', workload, route] for i in range(5)]
        changes = [(b/a-1)*100 for a, b in zip(baseline, prepared)]
        results.append(dict(workload=workload, route=route, pairs=5,
            baseline_ms=stats(baseline), prepared_ms=stats(prepared),
            ratio_of_medians_percent=(statistics.median(prepared)/statistics.median(baseline)-1)*100,
            paired_change_percent=stats(changes), improved_pairs=sum(b<a for a,b in zip(baseline,prepared))))

decision = dict(status='rejected', source_resolution='pending_removal_and_verification',
    candidate='batch-scoped Arrow column preparation, pvkslvyz / 0faa5171b133d3d9f3171b27d1089ab3babd8834',
    gate='Retain only repeatable targeted backend AND full COPY gains without unexplained material regressions in the other workloads.',
    rationale='The five complete pairs do not meet the retention gate. Broad rechecks improve, but small-text and wide-row backend/COPY medians worsen; narrow COPY also has a worse ratio of medians. Paired ranges and unchanged direct-client variation are large. This is insufficient evidence to retain the extra conversion machinery, not proof that the prototype is universally slower.',
    scope='Warm persistent clients on a shared host; in_band, unchanged 16 MiB individual batch allowance. Full projection/row counts preserved. Wide rows: 32768; other workloads: 1000000.',
    correctness='All ten runners exited zero and cleaned up. Complete direct-value controls precede timings on each connection; measured direct runs consume all batches and check rows. Every measured COPY validates every field and the complete unique row-ID set after its timer. Backend scans check complete row counts.',
    limitations=['Direct-client timings are an unchanged control, not a normalization factor or a beneficiary of FDW changes.',
        'The ratio of medians and median paired change answer different questions; both are retained.',
        'Native, decode, terminal and backend clocks overlap; they cannot be summed into exclusive stages.',
        'Module snapshots and binary hashes are preserved; full historical build-input attestation is unavailable.',
        'The earlier flush_first refusal remains separate; these successful in_band trials do not establish flush_first reliability.'],
    measurements=results)
(root / 'decision.json').write_text(json.dumps(decision, indent=2) + '\n')
print('FDW prototype rejected: retention gate unmet across five complete pairs; source removal/verification pending.')
