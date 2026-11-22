"""Read completed terminal-policy/batch-budget controls without replaying queries."""
import hashlib, json, statistics
from pathlib import Path
root = Path('.local/perf/fdw')
status = {r['step']:r for r in json.loads(Path('.local/perf/execution/remaining-status.json').read_text())}
pg_status_path=Path('.local/perf/execution/pg-budget-status.json')
pg_status={r['case']:r for r in json.loads(pg_status_path.read_text())} if pg_status_path.exists() else {}
cases = []
refusals = []
pending = []
stats = lambda xs: dict(median=statistics.median(xs), min=min(xs), max=max(xs)) if xs else None
for name, workload in [(f'policy-{p}', 'narrow') for p in ('flush_first','observe_first','observe_only','in_band')] + [(f'{prefix}budget-{b}', 'wide1k') for prefix in ('','pg-') for b in (1048576,4194304)]:
    is_pg_budget=name.startswith('pg-budget-')
    entry = pg_status.get(name) if is_pg_budget else status.get('fdw-' + name.removeprefix('policy-'))
    if entry is None:
        pending.append(name)
        continue
    directory = root / name
    assert json.loads((directory/'cleanup.json').read_text())['failures'] == []
    settings = json.loads((directory/'settings.json').read_text())
    assert settings['concurrency'] == 1 and settings['repeats'] == 3
    if entry['exit_status']:
        assert (name.startswith('budget-') and entry['classification'].startswith('expected_data_resource_refusal')) or (is_pg_budget and entry['classification'].startswith('expected PG data resource refusal'))
        private_log = directory/('pg-0.private.log' if is_pg_budget else 'direct-0.private.log')
        error = 'wildcard_fdw: Antalya Data: configured resource limit exceeded' if is_pg_budget else 'Context { phase: Data, cause: LimitExceeded }'
        proof_path = directory/'resource-refusal.json'
        if private_log.exists():
            detail = private_log.read_bytes()
            assert error.encode() in detail
            proof = dict(typed_error=error, source_log_sha256=hashlib.sha256(detail).hexdigest(),
                source='typed error extracted from original private worker stderr; no credentials or endpoint copied')
            proof_path.write_text(json.dumps(proof,indent=2)+'\n')
        else:
            proof = json.loads(proof_path.read_text())
            assert proof['typed_error'] == error
            assert len(proof['source_log_sha256']) == 64 and all(c in '0123456789abcdef' for c in proof['source_log_sha256'])
        refusals.append(dict(case=name, policy=settings['policy'], batch_bytes=settings['batch_bytes'],
            classification=('PG full-COPY precontrol' if is_pg_budget else 'direct value-control')+' data resource refusal before measured samples',
            typed_error=error, private_log_sha256=proof['source_log_sha256'], cleanup_confirmed=True))
        continue
    raw = [json.loads(line) for line in (directory/'raw.jsonl').read_text().splitlines()]
    cohorts = [r for r in raw if r.get('kind')=='cohort']
    summary = json.loads((directory/'summary.json').read_text())
    assert settings['routes']==(['pg_backend','pg_copy'] if is_pg_budget else ['direct','pg_backend','pg_copy'])
    for route in settings['routes']:
        selected = [r for r in cohorts if r['route']==route and not r['warmup']]
        assert len(selected)==3 and {r['repeat'] for r in selected}==set(range(3))
        assert sum(c['warmup'] for c in cohorts if c['route']==route)==1
        values = []; diagnostics=[]; backend_rss=[]
        for c in selected:
            assert c['workload']==workload and len(c['results'])==1
            r=c['results'][0]; rows=1000000 if workload=='narrow' else 32768
            values.append(r['request_ms'])
            if route=='direct':
                assert r['rows']==rows
                diagnostics.append(dict(first_batch_ms=r['first_batch_ms'], decode_ms=r['arrow_decode_ms'],
                    framing_ms=r['framing_decode_ms'], terminal_ms=r['terminal_ms'],
                    control_requests=r['control_requests'], peak_individual_batch_bytes=r['peak_individual_batch_bytes']))
            elif route=='pg_backend':
                p=r['plan']['Plan']; assert p['Actual Rows']==rows and p['FDW metrics complete']
                assert not p['FDW row timing enabled']
                diagnostics.append(dict(first_batch_ms=p['FDW first batch'], decode_ms=p['FDW Arrow decode'],
                    framing_ms=p['FDW framing decode'], terminal_ms=p['FDW terminal confirmation'],
                    control_requests=p['FDW data control requests'], peak_individual_batch_bytes=p['FDW peak individual batch bytes']))
            else:
                assert r['value_control']['rows']==rows and r['value_control']['oracle']=='every field and complete unique row-ID set'
            if route.startswith('pg_'):
                backend_rss.extend(p['rss_kib']*1024 for s in c['samples'] for p in s['processes'])
        for d in diagnostics:
            assert d['peak_individual_batch_bytes'] <= settings['batch_bytes']
        observed=stats(values); stated=next(s for s in summary if s['route']==route)
        assert stated['samples']==3 and stated['median_ms']==observed['median']
        assert stated['min_ms']==observed['min'] and stated['max_ms']==observed['max']
        cases.append(dict(case=name, workload=workload, route=route, policy=settings['policy'],
            batch_bytes=settings['batch_bytes'], samples=3, request_ms=observed,
            executor_ms=stats([c['results'][0]['executor_ms'] for c in selected]) if route=='pg_backend' else None,
            diagnostics={k:stats([d[k] for d in diagnostics]) for k in diagnostics[0]} if diagnostics else {},
            max_sampled_backend_rss_bytes=max(backend_rss,default=None),
            monitor_errors=[e for c in selected for e in c['monitor_errors']]))
lines=['# FDW terminal policy and batch-budget controls','',
    'Baseline release library; persistent connections; one excluded warmup and '
    'three measured samples per completed route. Narrow is one million integer '
    'rows; wide is 32,768 rows with a 1 KiB payload. Policy tests use 16 MiB; '
    'budget tests use in_band. These are small diagnostic controls, not five-pair '
    'optimization promotion experiments. Complete Arrow values are checked before '
    'timing, full COPY values after timing; backend scans verify row counts.','',
    '| Case | Route | Request ms median [range] | Executor ms median [range] | First batch ms median | Terminal ms median | Control requests median | Largest observed individual batch bytes |',
    '|---|---|---:|---:|---:|---:|---:|---:|']
fmt=lambda v:'—' if v is None else f"{v['median']:.3f} [{v['min']:.3f}, {v['max']:.3f}]"
for r in cases:
    d=r['diagnostics']; metric=lambda k:'—' if k not in d else f"{d[k]['median']:.3f}"
    peak='—' if not d else str(d['peak_individual_batch_bytes']['max'])
    lines.append(f"| {r['case']} | {r['route']} | {fmt(r['request_ms'])} | {fmt(r['executor_ms'])} | {metric('first_batch_ms')} | {metric('terminal_ms')} | {metric('control_requests')} | {peak} |")
lines += ['','First batch is not network time to first byte. Decode, framing, polling, '
    'terminal and backend clocks overlap and are not additive stages. COPY does '
    'not expose the backend EXPLAIN diagnostics; its cells remain unavailable. '
    'The measured per-batch charge is checked against its configured limit. This '
    'does not assert aggregate retained allocations or backend RSS fit within '
    'one batch allowance. Sampled backend RSS is retained separately in JSON.', '',
    '## Explicit resource refusals', '']
lines.extend('- '+json.dumps(r,sort_keys=True) for r in refusals)
if not refusals:lines.append('None among the completed controls.')
if pending:lines+=['','Pending: '+', '.join(pending)+'.']
(root/'controls-report.md').write_text('\n'.join(lines)+'\n')
(root/'controls-report.json').write_text(json.dumps(dict(cases=cases,refusals=refusals,pending=pending),indent=2)+'\n')
print('FDW controls:',len(cases),'completed route rows,',len(refusals),'refusals,',len(pending),'pending cases')
