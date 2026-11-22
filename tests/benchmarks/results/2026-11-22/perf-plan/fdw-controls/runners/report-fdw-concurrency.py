"""Completed cohort throughput/resources, preserving hierarchy of repetitions."""
import json
import statistics
from pathlib import Path

root = Path('.local/perf/fdw')
groups = {1: [root / f'inband-baseline-{i}' for i in range(5)],
          8: [root / 'concurrency-8'], 32: [root / 'concurrency-32']}
pending = []
output = []
lines = ['# FDW concurrency observations', '',
    'C1 contains five separate baseline rounds. C8 and C32 each contain one '
    'measured barrier-started cohort per workload/route after a warmup; clients '
    'within a cohort are not independent repetitions. This is an observed '
    'scaling/resource control, not a five-repetition concurrency promotion gate. '
    'Rows are fully consumed. Wide means 32,768 rows of 1 KiB payload per client; '
    'narrow means one million integer rows per client.', '',
    '| Requested clients | Workload | Route | Measured cohorts | Client samples | Client request ms median [range] | Cohort rows/s median [range] | Observed clients / database-referencing queries | Largest backend RSS MiB | Largest sampled sum of backend RSS MiB | Largest sampled sum of backend threads |',
    '|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|']
stats = lambda xs: dict(median=statistics.median(xs), min=min(xs), max=max(xs))
cell = lambda s: f"{s['median']:.1f} [{s['min']:.1f}, {s['max']:.1f}]"
for concurrency, directories in groups.items():
    if any(not (directory / 'summary.json').exists() for directory in directories):
        pending.append(concurrency)
        continue
    cohorts = []
    for directory in directories:
        if json.loads((directory / 'cleanup.json').read_text())['failures']:
            raise ValueError('unclean run')
        records = [json.loads(line) for line in (directory / 'raw.jsonl').read_text().splitlines()]
        cohorts.extend(r for r in records if r.get('kind') == 'cohort' and not r['warmup'])
    for workload in ('narrow', 'wide1k'):
        for route in ('direct', 'pg_backend', 'pg_copy'):
            selected = [r for r in cohorts if r['workload'] == workload and r['route'] == route]
            if len(selected) != (5 if concurrency == 1 else 1):
                raise ValueError('incomplete or duplicate cohort')
            rows = 32768 if workload == 'wide1k' else 1000000
            requests = []
            rates = []
            rss = []
            summed_rss = []
            summed_threads = []
            for cohort in selected:
                results = cohort['results']
                if cohort['requested_concurrency'] != concurrency or len(results) != concurrency or {r['worker'] for r in results} != set(range(concurrency)):
                    raise ValueError('worker/concurrency mismatch')
                for result in results:
                    actual = (result['rows'] if route == 'direct' else
                              result['plan']['Plan']['Actual Rows'] if route == 'pg_backend' else
                              result['value_control']['rows'])
                    if actual != rows:
                        raise ValueError('incomplete consumption')
                    requests.append(result['request_ms'])
                rates.append(rows * concurrency * 1000 / cohort['cohort_ms'])
                if route.startswith('pg_'):
                    for sample in cohort['samples']:
                        processes = sample['processes']
                        if len({p['pid'] for p in processes}) != len(processes):
                            raise ValueError('duplicate process sample')
                        if processes:
                            rss.extend(p['rss_kib'] / 1024 for p in processes)
                            summed_rss.append(sum(p['rss_kib'] for p in processes) / 1024)
                            summed_threads.append(sum(p['threads'] for p in processes))
            native_peaks = [r['observed_engine_peak'] for r in selected if r['observed_engine_peak'] is not None]
            record = dict(concurrency=concurrency, workload=workload, route=route,
                measured_cohorts=len(selected), client_samples=len(requests), client_ms=stats(requests),
                cohort_rows_per_second=stats(rates), observed_client_peak=max(r['observed_client_peak'] for r in selected),
                observed_database_reference_query_peak=max(native_peaks, default=None),
                sampled_max_backend_rss_mib=max(rss, default=None),
                sampled_max_sum_backend_rss_mib=max(summed_rss, default=None),
                sampled_max_sum_backend_threads=max(summed_threads, default=None),
                monitor_errors=[e for r in selected for e in r['monitor_errors']])
            output.append(record)
            fmt = lambda v: '—' if v is None else f'{v:.1f}'
            lines.append(f"| {concurrency} | {workload} | {route} | {len(selected)} | {len(requests)} | "
                f"{cell(record['client_ms'])} | {cell(record['cohort_rows_per_second'])} | "
                f"{record['observed_client_peak']} / {record['observed_database_reference_query_peak']} | "
                f"{fmt(record['sampled_max_backend_rss_mib'])} | {fmt(record['sampled_max_sum_backend_rss_mib'])} | "
                f"{fmt(record['sampled_max_sum_backend_threads'])} |")
lines += ['', 'Throughput divides complete cohort rows by earliest client start to '
    'last client completion. Request clocks retain each route\'s existing boundary. '
    'The server sampler counts query text referencing this invocation\'s database '
    'and excludes itself. It can include diagnostic queries, so it is not an exact '
    'source-scan concurrency measure: the C32 wide backend control observes 33 '
    'database-referencing queries while only 32 client requests are outstanding. '
    'The raw count is retained rather than capped or relabeled as 33 clients. '
    'Resource peaks are sampled observations, not proven absolute maxima. '
    'Backend RSS sums use processes from the same sample; shared '
    'pages can be counted multiple times, so they are not physical memory totals. '
    'Direct-client process RSS/threads were not instrumented; idle PostgreSQL '
    'processes are deliberately excluded from that route\'s resource cells. '
    'Neither RSS measure is bounded by the individual batch allowance. '
    'Raw settings, active-query samples, per-backend samples and query profiles '
    'remain in each run directory. No clocks are subtracted into exclusive stages.']
if pending:
    lines += ['', 'Pending complete runs: ' + ', '.join(map(str, pending)) + ' clients.']
interruption_path = root / 'concurrency-32-interrupted-reboot/interruption.json'
interruption = json.loads(interruption_path.read_text()) if interruption_path.exists() else None
if interruption:
    lines += ['', 'The original C32 attempt was interrupted by a host reboot and '
        'is preserved separately with its damaged tail and incomplete metadata. '
        'Only the fresh, fully validated C32 run appears in this table. C1/C8 were '
        'measured before that reboot and C32 afterwards, on a shared host. These '
        'are separate scaling observations, not a controlled single-session '
        'scaling curve or proof that a throughput difference is caused only by '
        'concurrency. No cold-cache claim is made.']
(root / 'concurrency-report.md').write_text('\n'.join(lines) + '\n')
(root / 'concurrency-report.json').write_text(json.dumps(dict(cases=output, pending=pending, interrupted_attempt=interruption), indent=2) + '\n')
print(root / 'concurrency-report.md', 'pending', pending)
