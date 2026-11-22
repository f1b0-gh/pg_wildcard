"""Recover invocation-scoped native exceptions after all measurements end."""
import hashlib
import json
import re
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

root=Path('.local/perf')
assert not (root/'execution/timing-active').exists(), 'timing window remains active'
controller=json.loads((root/'execution/resumed-controller.json').read_text())
proc=Path(f"/proc/{controller['pid']}/stat")
if proc.exists():
    value=proc.read_text()
    assert value[value.rfind(')')+2:].split()[0] in ('Z','X'), 'controller remains live'
budgets=json.loads((root/'execution/pg-budget-status.json').read_text())
assert {r['case'] for r in budgets}=={'pg-budget-1048576','pg-budget-4194304'}
assert all(r['exit_status']==0 or r['classification'].startswith('expected PG data resource refusal') for r in budgets)
target=root/'matrix-native-failures.json'
assert not target.exists(), 'refusing to replace recovered evidence'
state=json.loads((root/'antalya.json').read_text())
opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))


def query(sql):
    params=dict(database='default',default_format='JSONCompact',log_queries=0,
        max_execution_time=30,max_memory_usage=134217728,max_result_rows=1000,
        max_result_bytes=16777216,result_overflow_mode='throw',query_id='wc-evidence-'+uuid.uuid4().hex)
    request=urllib.request.Request(state['url']+'/?'+urllib.parse.urlencode(params),
        data=sql.encode(),headers={'Authorization':'Bearer '+state['token']})
    with opener.open(request,timeout=40) as response:
        data=response.read(16777217)
        assert len(data)<=16777216
    return json.loads(data)['data'] if data.strip() else []


status=json.loads((root/'execution/matrix-status.json').read_text())
latest={r['case']:r for r in status}
directories=[root/'matrix'/name for name,r in latest.items() if r['status']=='complete']
directories += [Path(r['attempt_directory']) for r in status if 'attempt_directory' in r]
query('SYSTEM FLUSH LOGS')
cases=[]
columns=['query_id','type','query_duration_ms','read_rows','read_bytes','memory_usage',
    'exception_code','exception','query','settings']
for directory in directories:
    raw=[json.loads(l) for l in (directory/'raw.jsonl').read_text().splitlines()]
    errors=[dict(route=r.get('route'),warmup=r.get('warmup'),repeat=r.get('repeat'),worker=v.get('worker'),error=v['error'])
        for r in raw if r.get('kind')=='cohort' for v in r['results'] if 'error' in v]
    prechecks=[r for r in raw if r.get('kind')=='precontrol_failure']
    setup=json.loads((directory/'failure.json').read_text()) if (directory/'failure.json').exists() else None
    if not errors and not prechecks and not setup:continue
    sqls=json.loads((directory/'queries.json').read_text())
    names=set(re.findall(r'wc_benchmark_[0-9a-f]{32}',json.dumps(sqls)))
    assert len(names)==1
    database=names.pop()
    sql=("SELECT query_id,toString(type),query_duration_ms,read_rows,read_bytes,memory_usage,"
        "exception_code,exception,query,Settings FROM system.query_log WHERE is_initial_query=1 "
        "AND type IN ('ExceptionBeforeStart','ExceptionWhileProcessing') "
        f"AND (current_database='{database}' OR position(query,'{database}')>0) "
        f"AND (startsWith(query_id,'{database}_') OR startsWith(query_id,'comparison-direct-') "
        "OR startsWith(query_id,'wildcard-')) ORDER BY event_time_microseconds LIMIT 1000")
    records=query(sql)
    assert len(records)<1000, 'possibly truncated exception recovery'
    cases.append(dict(directory=str(directory),database=database,client_errors=errors,
        precontrol_failures=prechecks,setup_failure=setup,
        raw_sha256=hashlib.sha256((directory/'raw.jsonl').read_bytes()).hexdigest(),
        native_exceptions=[dict(zip(columns,r)) for r in records],recovery_query=sql))
output=dict(scope='original server exceptions scoped to owned invocations; includes setup/prechecks/warmups, not exclusive stage timings or a per-sample mapping',
    provenance='post-run recovery without replay; absence of a server record does not identify the client error cause; cancellation records may reflect cleanup after a client failure rather than its original cause',cases=cases)
encoded=(json.dumps(output,indent=2)+'\n').encode()
assert state['token'].encode() not in encoded and state['url'].encode() not in encoded
target.write_bytes(encoded)
print('Recovered',sum(len(c['native_exceptions']) for c in cases),'original native exceptions across',len(cases),'invocations')
