"""Consolidate completed matrix index evidence; no engine calls or new timings."""
import json
import statistics
from pathlib import Path

root = Path('.local/perf')
matrix = json.loads((root / 'matrix-report.json').read_text())
storage = json.loads((root / 'matrix-storage.json').read_text())
status = json.loads((root / 'execution/matrix-status.json').read_text())
assert set(matrix['completed_cases']) == {r['case'] for r in status if r['status'] == 'complete'}


def stats(values):
    return dict(median=statistics.median(values), min=min(values), max=max(values)) if values else None


def fmt(value, divisor=1):
    return '—' if value is None else f"{value['median']/divisor:,.3f} [{value['min']/divisor:,.3f}, {value['max']/divisor:,.3f}]"


queries, costs = [], []
for case in matrix['completed_cases']:
    rows = [r for r in matrix['cases'] if r['case'] == case]
    fixture = next(r for r in matrix['fixtures'] if r['case'] == case)
    raw = [json.loads(line) for line in (root / 'matrix' / case / 'raw.jsonl').read_text().splitlines()]
    for row in rows:
        if row['route'] not in ('native', 'indexed', 'elastic_wildcard', 'elastic_keyword'):
            continue
        marks, query_counts = [], []
        for cohort in raw:
            if cohort.get('kind') != 'cohort' or cohort['route'] != row['route'] or cohort['warmup']:
                continue
            if any('error' in r for r in cohort['results']):
                continue
            profiles = cohort.get('profiles', []) + [p for r in cohort['results'] for p in r.get('profiles', [])]
            if profiles:
                query_counts.append(len(profiles) / row['concurrency'])
                if all('SelectedMarks' in p['profile_events'] for p in profiles):
                    marks.append(sum(p['profile_events']['SelectedMarks'] for p in profiles) / row['concurrency'])
        reads = row['native_total_cohort']['read_rows']
        queries.append(dict(case=case, route=row['route'], concurrency=row['concurrency'],
            censored=row['censored'], complete_cohorts=row['complete_cohorts'],
            request_ms=row['request_ms'], logical_rows_per_client_cohort_mean=(
                {k: v / row['concurrency'] for k, v in reads.items()} if reads else None),
            selected_marks_per_client_cohort_mean=stats(marks),
            logged_queries_per_client_cohort_mean=stats(query_counts),
            largest_query_bytes=row['largest_native_query_bytes'],
            indicator_entries=fixture['indicator_entries'], unique_regex_view_pairs=fixture['unique_regex_view_pairs']))
    builds = [r for r in storage if r['case'] == case and r['kind'] == 'index_build']
    assert len(builds) <= 1
    for source in (r for r in storage if r['case'] == case and r['kind'] in ('source', 'elastic_source')):
        assert source['corpus_sha256'] == rows[0]['corpus_sha256']
        elastic = source['kind'] == 'elastic_source'
        measured = source['storage']['_all']['primaries'] if elastic else source['storage']
        assert (measured['docs']['count'] if elastic else measured['rows']) == fixture['rows']
        layout = 'elastic_' + source['field_type'] if elastic else source['table']
        costs.append(dict(case=case, layout=layout, rows=fixture['rows'],
            initial_rows=fixture['rows'] * 9 // 10, incremental_rows=fixture['rows'] - fixture['rows'] * 9 // 10,
            initial_load_seconds=source['initial_load_seconds'], incremental_insert_seconds=source['incremental_insert_seconds'],
            index_backfill_seconds=builds[0]['seconds'] if layout == 'bench_indexed' and builds else None,
            total_bytes=measured['store']['size_in_bytes'] if elastic else measured['total_bytes'],
            data_compressed_bytes=None if elastic else measured['data_compressed_bytes'],
            index_compressed_bytes=None if elastic else measured['index_compressed_bytes']))

lines = ['# Index query, write and storage tradeoffs', '',
    'Completed cases only; the full planned matrix is not complete until every case in '
    'matrix-status.json has a terminal outcome. Exact pairs and summary clocks are '
    'validated by report-matrix.py before this report is generated. See '
    '[the route matrix](matrix-report.md) for direct antalya-rs, PostgreSQL backend '
    'and full COPY measurements; they are distinct routes.', '',
    '## Query observations', '',
    'Each uncensored route has five measured cohorts after warmup. Request times '
    'are client samples; at C4 they are twenty clients nested in five cohorts. '
    'Reads, selected marks and query counts are totals per complete cohort divided '
    'by its client count, then summarized across cohorts. These are query-log logical '
    'reads, not measured disk traffic. Missing counters remain missing. Elastic '
    'wildcard and keyword rows have no invented native read counters.', '',
    '| Case | Route | Request seconds median [range] | Logical rows/client median [range] | Selected marks/client median [range] | Logged queries/client median [range] | Complete cohorts |',
    '|---|---|---:|---:|---:|---:|---:|']
for row in queries:
    label = row['route'] + (' (censored)' if row['censored'] else '')
    lines.append(f"| {row['case']} | {label} | {fmt(row['request_ms'], 1000)} | {fmt(row['logical_rows_per_client_cohort_mean'])} | {fmt(row['selected_marks_per_client_cohort_mean'])} | {fmt(row['logged_queries_per_client_cohort_mean'])} | {row['complete_cohorts']} |")
lines += ['',
    'An index appearing in EXPLAIN does not prove useful pruning. Compare actual '
    'rows and marks; a query can read the same marks and still differ in CPU/time. '
    'The indexed layout also uses materialized view columns, so generated SQL and '
    'batch counts can differ. Fewer total reads caused by fewer source queries '
    'must not be attributed to index pruning. For the completed mixed-1000 C1 '
    'case, native uses seven queries and indexed native five; all 35 native and '
    '25 indexed measured query profiles read one million rows and 123 marks each. '
    'These are complete layout comparisons, not an isolated index-on/off control. '
    'Censored routes do not establish a complete five-cohort performance comparison. '
    'Fixtures are compared within each case only. In particular, short/no-gram '
    'fixtures repeat one literal under multiple IDs, and mixed views have no Elastic '
    'equivalence claim. Keyword is a historical storage control, not an admitted '
    'application keyword field.', '',
    '## Setup and insertion observations', '',
    'These are one setup observation per layout and case, not five repeated write '
    'trials. Native initial load creates 90% of rows; indexed setup copies those '
    'rows (copy time was not recorded), then adds/materializes its index. Backfill '
    'includes materialized view columns where specified. Incremental insertion '
    'adds the remaining 10%, followed by OPTIMIZE FINAL outside the insert clock. '
    'Elastic bulk clocks include client generation/serialization and HTTP writes; '
    'refresh and force-merge occur afterwards. They are not exclusive engine write '
    'times and are not directly comparable to native INSERT SELECT CPU.', '',
    '| Case | Layout | Initial load s | Index backfill s | Incremental insert s | Total bytes | Data compressed bytes | Index compressed bytes |',
    '|---|---|---:|---:|---:|---:|---:|---:|']
number = lambda value: '—' if value is None else f'{value:,.3f}' if isinstance(value, float) else f'{value:,}'
for row in costs:
    lines.append('| ' + ' | '.join([row['case'], row['layout']] + [number(row[k]) for k in (
        'initial_load_seconds', 'index_backfill_seconds', 'incremental_insert_seconds', 'total_bytes', 'data_compressed_bytes', 'index_compressed_bytes')]) + ' |')
lines += ['',
    'Total storage describes whole layouts, not comparable isolated index overhead. '
    'Native mixed-view data includes additional materialized columns; the difference '
    'in total bytes cannot all be attributed to the index. Exact DDL and raw engine '
    'statistics remain in each case directory and matrix-storage.json.', '',
    'The separately reported bounded raw-byte index is a different private layout. '
    'Its five paired write trials must not be substituted for these production '
    'text-index observations. The current evidence does not justify automatically '
    'migrating every wildcard field to that prototype.', '',
    'Shared-host load and filesystem caches were uncontrolled; retain the reported '
    'ranges, session boundaries and settings rather than treating small differences '
    'as a general engine ranking.']
(root / 'index-tradeoffs.md').write_text('\n'.join(lines) + '\n')
(root / 'index-tradeoffs.json').write_text(json.dumps(dict(completed_cases=matrix['completed_cases'], query_observations=queries, setup_observations=costs), indent=2) + '\n')
print('Validated index tradeoffs:', len(queries), 'query rows and', len(costs), 'setup observations')
