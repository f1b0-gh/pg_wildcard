"""Preserve recorded confirmation diagnostics without subtracting route clocks."""
import hashlib
import json
from pathlib import Path

root = Path('.local/perf')
directory = root / 'matrix/domains-current-100-c1'
raw = (directory / 'raw.jsonl').read_bytes()
records = [json.loads(line) for line in raw.splitlines()]
spec = json.loads((directory / 'spec.json').read_text())
samples = []


def foreign_nodes(node):
    if node.get('Node Type') == 'Foreign Scan':
        yield node
    for child in node.get('Plans', []):
        yield from foreign_nodes(child)


for cohort in records:
    if cohort.get('kind') != 'cohort' or cohort['warmup']:
        continue
    if cohort['route'] not in ('native', 'direct', 'pg_backend'):
        continue
    assert len(cohort['results']) == 1
    result = cohort['results'][0]
    assert 'error' not in result
    record = dict(route=cohort['route'], repeat=cohort['repeat'], request_ms=result['request_ms'])
    if cohort['route'] == 'direct':
        assert len(result['executions']) == 1
        record['recorded_execution'] = result['executions'][0]
    elif cohort['route'] == 'pg_backend':
        nodes = list(foreign_nodes(result['plan']['Plan']))
        assert len(nodes) == 1 and nodes[0]['FDW metrics complete']
        node = nodes[0]
        record['executor_ms'] = result['executor_ms']
        record['recorded_execution'] = {k: node[k] for k in (
            'FDW last query ID', 'FDW first batch', 'FDW Arrow decode',
            'FDW framing decode', 'FDW terminal confirmation', 'FDW data control requests')}
    samples.append(record)
for route in ('native', 'direct', 'pg_backend'):
    selected = [s for s in samples if s['route'] == route]
    assert len(selected) == 5 and {s['repeat'] for s in selected} == set(range(5))
output = dict(case=directory.name, rows=spec['rows'], indicator_entries=len(spec['patterns']),
    raw_sha256=hashlib.sha256(raw).hexdigest(),
    scope='recorded diagnostics from five completed samples per selected route; no query replay',
    interpretation='terminal/body/decode/request clocks can overlap; no subtraction into exclusive stages; first batch is not network TTFB; COPY has no equivalent EXPLAIN diagnostics here',
    follow_up='https://github.com/f1b0-gh/pg_wildcard/issues/19', samples=samples)
(root / 'domain-terminal-observation.json').write_text(json.dumps(output, indent=2) + '\n')
print('Validated terminal observations for', len(samples), 'completed route samples')
