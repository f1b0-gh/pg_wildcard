"""Retain both initial-order cohorts; never replace the first regression."""
import json
import statistics
from pathlib import Path

root = Path('.local/perf/percolator')
directories = [root / 'production-confirmation-1000/unrelated',
               root / 'production-confirmation-unrelated-reverse/unrelated']
labels = ('prototype_join', 'production_join')
cohorts = []
reference = None
for index, directory in enumerate(directories):
    settings = json.loads((directory / 'settings.json').read_text())
    if (settings['rows'], settings['indicators'], settings['repeats']) != (1000000, 1000, 5):
        raise ValueError('different workload size')
    if bool(settings.get('reverse_order', False)) != bool(index):
        raise ValueError('initial order was not reversed')
    # Equality includes the exact raw corpus, patterns, generated statements,
    # views, engine revision and execution settings. Machine load is reported,
    # not required to stay constant on this shared development host.
    identity = {name: json.loads((directory / name).read_text())
                for name in ('spec.json', 'variants.json', 'diagnostics.json')}
    identity.update(engine=settings['engine'], settings=settings['settings'])
    if reference is not None and identity != reference:
        raise ValueError('cohorts differ in corpus, SQL, implementation or settings')
    reference = identity
    if not (directory / 'summary.json').exists() or (directory / 'pending.json').exists():
        raise ValueError('cohort is incomplete')
    raw = [json.loads(line) for line in (directory / 'raw.jsonl').read_text().splitlines()]
    raw = [r for r in raw if 'strategy' in r]
    warmups = [r for r in raw if r['warmup']]
    measured = [r for r in raw if not r['warmup']]
    if len(warmups) != 2 or {r['strategy'] for r in warmups} != set(labels):
        raise ValueError('warmup coverage differs')
    slots = {(r['strategy'], r['repeat']): r for r in measured}
    if len(measured) != 10 or set(slots) != {(label, rep) for label in labels for rep in range(5)}:
        raise ValueError('missing or duplicate paired measurements')
    if len({(r['pairs'], r['pair_sha256']) for r in raw}) != 1:
        raise ValueError('within-cohort oracle mismatch')
    cohorts.append({'directory': str(directory), 'slots': slots})

all_rows = [r for cohort in cohorts for r in cohort['slots'].values()]
if len({(r['pairs'], r['pair_sha256']) for r in all_rows}) != 1:
    raise ValueError('between-cohort oracle mismatch')

def summarize(selected):
    groups = [[c['slots'][label, rep] for c in selected for rep in range(5)] for label in labels]
    times = [[r['request_ms'] / 1000 for r in group] for group in groups]
    paired = [(b / a - 1) * 100 for a, b in zip(*times)]
    stats = lambda values: dict(median=statistics.median(values), min=min(values), max=max(values))
    return dict(pairs=len(times[0]),
        seconds={label: stats(values) for label, values in zip(labels, times)},
        median_ratio_percent=(statistics.median(times[1]) / statistics.median(times[0]) - 1) * 100,
        paired_change_percent=stats(paired),
        cpu_seconds={label: stats([sum(r['profile']['profile_events'].get(k, 0)
            for k in ('UserTimeMicroseconds', 'SystemTimeMicroseconds')) / 1e6 for r in group])
            for label, group in zip(labels, groups)},
        max_query_bytes={label: max(r['profile']['memory_usage'] for r in group)
                         for label, group in zip(labels, groups)})

results = {'original': summarize(cohorts[:1]), 'reverse': summarize(cohorts[1:]),
           'all_samples': summarize(cohorts)}
combined = results['all_samples']
results['investigation_required'] = combined['median_ratio_percent'] > 10
results['source_directories'] = [c['directory'] for c in cohorts]
results['pairs_per_insertion'] = all_rows[0]['pairs']
results['pair_sha256'] = all_rows[0]['pair_sha256']
lines = ['# Production transfer: balanced initial order', '',
    'Native MV insertion of 1M rows with 1,000 unrelated rules. Original and reverse-start '
    'cohorts are both retained. Warmups are excluded; all ten measured pairs contribute. '
    'This compares prototype and production SQL, not regex engines or staged-baseline speedup.', '',
    '| Cohort | Pairs | Prototype median [range] s | Production median [range] s | Ratio of medians | Paired change median [range] |',
    '|---|---:|---:|---:|---:|---:|']
cell = lambda s: f"{s['median']:.3f} [{s['min']:.3f}, {s['max']:.3f}]"
for name in ('original', 'reverse', 'all_samples'):
    r = results[name]
    lines.append(f"| {name} | {r['pairs']} | {cell(r['seconds'][labels[0]])} | "
        f"{cell(r['seconds'][labels[1]])} | {r['median_ratio_percent']:+.2f}% | "
        f"{cell(r['paired_change_percent'])}% |")
lines += ['', 'The original +11.85% ratio remains part of this evidence. '
    'Balancing the initial order reduces that specific ordering imbalance; it does not '
    'prove that all differences came from noise or control unrelated host activity.', '',
    'Investigation still required: ' + str(results['investigation_required']) + '. '
    'This diagnostic flag is not a production-promotion decision; correctness, updates, '
    'fallbacks, memory, other distributions and whole-change review remain separate gates.']
(root / 'production-balanced.json').write_text(json.dumps(results, indent=2) + '\n')
(root / 'production-balanced.md').write_text('\n'.join(lines) + '\n')
print(json.dumps(results, indent=2))
