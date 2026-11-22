"""Summarize actual rule updates separately from subsequent ingestion."""
import json
import statistics
from pathlib import Path

root = Path('.local/perf/percolator/update-cost')
if json.loads((root / 'completion.json').read_text()) != {'exit_status': 0, 'cases': 4}:
    raise ValueError('update experiment incomplete')
lines = ['# Watchlist update and subsequent ingestion costs', '',
    'Each trial starts from the same watchlist and rotates existing rule definitions '
    'among stable IDs. The global pattern set is unchanged. The update clock covers '
    'native HTTP upserts in batches of 128, excluding SQL generation. A separate '
    'clock measures the first resident-source insertion after visibility validation, '
    'including source and hit persistence. Neither clock is a PostgreSQL or '
    'direct antalya-rs measurement. No Elastic percolator comparison is made.', '',
    '| Family | Rules | Requested block rows | Strategy | Update ms median [range] | Rules/s | Subsequent 10k-row insert ms median [range] | Rows/s | Update CPU ms median | Insert CPU ms median | Largest update / insert query MiB |',
    '|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|']
cases = []
cpu = lambda p: sum(int(p['profile_events'].get(k, 0)) for k in
                   ('UserTimeMicroseconds', 'SystemTimeMicroseconds')) / 1000
stats = lambda values: dict(median=statistics.median(values), min=min(values), max=max(values))
cell = lambda s: f"{s['median']:.2f} [{s['min']:.2f}, {s['max']:.2f}]"
for family, count, block in [('domains', 100, 1024), ('domains', 100, 8192),
                             ('shared', 1000, 1024), ('shared', 1000, 8192)]:
    directory = root / f'{family}-{count}-b{block}'
    settings = json.loads((directory / 'settings.json').read_text())
    if (settings['rows'], settings['indicators'], settings['requested_block_rows'], settings['repeats']) != (10000, count, block, 5):
        raise ValueError('unexpected experiment settings')
    if not json.loads((directory / 'cleanup.json').read_text())['completed']:
        raise ValueError('incomplete experiment')
    if json.loads((directory / 'delete-reinsert-control.json').read_text())['status'] != 'PASS':
        raise ValueError('physical deletion control failed')
    raw = [json.loads(line) for line in (directory / 'updates.jsonl').read_text().splitlines()]
    measured = [r for r in raw if not r['warmup']]
    labels = ('staged_needles', 'production_join')
    slots = {(r['strategy'], r['repeat']): r for r in measured}
    if len(measured) != 10 or set(slots) != {(label, rep) for label in labels for rep in range(5)}:
        raise ValueError('missing or duplicate measured pair')
    for rep in range(5):
        pair = [slots[label, rep] for label in labels]
        # Different repetitions intentionally assign patterns to different IDs.
        if len({(r['shift'], r['pairs'], r['pair_sha256']) for r in pair}) != 1:
            raise ValueError('paired update output disagrees')
    case = dict(family=family, rules=count, requested_block_rows=block, strategies={})
    for label in labels:
        records = [slots[label, rep] for rep in range(5)]
        if any(len(r['update_profiles']) != (count + 127) // 128 for r in records):
            raise ValueError('incomplete update profiles')
        update = stats([r['update_ms'] for r in records])
        insert = stats([r['post_update_insert_ms'] for r in records])
        update_cpu = stats([sum(cpu(p) for p in r['update_profiles']) for r in records])
        insert_cpu = stats([cpu(r['insert_profile']) for r in records])
        update_peak = max(int(p['memory_usage']) for r in records for p in r['update_profiles'])
        insert_peak = max(int(r['insert_profile']['memory_usage']) for r in records)
        case['strategies'][label] = dict(update_ms=update, insert_ms=insert,
            update_cpu_ms=update_cpu, insert_cpu_ms=insert_cpu,
            largest_update_query_bytes=update_peak, largest_insert_query_bytes=insert_peak,
            insert_read_rows=[r['insert_profile']['read_rows'] for r in records])
        lines.append(f"| {family} | {count} | {block} | {label} | {cell(update)} | "
            f"{count * 1000 / update['median']:.0f} | {cell(insert)} | "
            f"{10000000 / insert['median']:.0f} | {update_cpu['median']:.2f} | "
            f"{insert_cpu['median']:.2f} | {update_peak / 1048576:.2f} / {insert_peak / 1048576:.2f} |")
    cases.append(case)
lines += ['', 'Five interleaved measured pairs per case; warmups excluded. Complete '
    'row/indicator/view multisets agree after every update and subsequent insertion. '
    'Requested block settings are recorded, not a measured distribution of internal '
    'block sizes. Throughput divides the fixed operation count by its median latency. '
    'CPU sums each update request within one trial; memory is the largest individual '
    'query peak, not total process RSS. Both strategies write the same watchlist '
    'schema; any update-latency difference is a control, not evidence of a cheaper '
    'update algorithm. New-pattern compilation and concurrent update visibility '
    'require different experiments. Raw query profiles, SQL, settings and snapshots '
    'remain beside each case.']
(root / 'report.md').write_text('\n'.join(lines) + '\n')
(root / 'report.json').write_text(json.dumps(cases, indent=2) + '\n')
print(root / 'report.md')
