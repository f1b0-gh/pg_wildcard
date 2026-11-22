# SPDX-License-Identifier: Apache-2.0
"""Bulk post-cohort correlation; never flush query logs between timed requests."""
import copy
import json
import re
import time


def selected_sql(query):
    return query.rstrip().removesuffix('\nFORMAT ArrowStream').rstrip().removesuffix(';').rstrip()


def bind_profiles(records, logs):
    by_id={row['query_id']:row for row in logs}
    if len(by_id)!=len(logs):raise ValueError('duplicate QueryFinish records')
    result=copy.deepcopy(records)
    if any(r['status']!='success' for r in result):raise ValueError('cohort includes failed requests')
    known={r['query_id'] for r in result if 'query_id' in r}
    if len(known)!=sum('query_id' in r for r in result):raise ValueError('duplicate measured query IDs')
    copies=[r for r in result if r['route']=='pg_copy']
    unknown=[r for r in logs if r['query_id'] not in known]
    if len(unknown)!=len(copies):raise ValueError('COPY query-log correlation is ambiguous')
    for record,row in zip(copies,unknown):
        matches=[r for r in result if r['route']=='pg_backend' and all(
            r[k]==record[k] for k in ('workload','policy','block_size','repeat'))]
        if len(matches)!=1 or selected_sql(row['query'])!=selected_sql(matches[0]['selected_sql']):
            raise ValueError('COPY selected SQL differs from paired backend SQL')
        record.update(query_id=row['query_id'],selected_sql=row['query'])
    for record in result:
        row=by_id.get(record.get('query_id'))
        if row is None:raise ValueError('missing QueryFinish record')
        if int(row['result_rows'])!=record['rows']:raise ValueError('query-log row count differs')
        if str(row['settings'].get('max_block_size'))!=str(record['block_size']):
            raise ValueError('executed block setting differs')
        if row['settings'].get('output_format_arrow_compression_method')!='none':
            raise ValueError('query-log compression differs from uncompressed experiment contract')
        record['profile']={k:v for k,v in row.items() if k not in ('query_id','query')}
    return result


def collect_profiles(engine, records, output):
    ids=[r['query_id'] for r in records if 'query_id' in r]
    if not ids or any(not re.fullmatch(r'[A-Za-z0-9_-]{1,128}',qid) for qid in ids):
        raise ValueError('invalid measured query IDs')
    if not re.fullmatch(r'wc_benchmark_[0-9a-f]{32}',engine.database):
        raise ValueError('profile collection requires an owned database')
    names=['query_id','query','query_duration_ms','read_rows','read_bytes','result_rows',
           'result_bytes','memory_usage','profile_events','settings']
    sql="SELECT query_id,query,query_duration_ms,read_rows,read_bytes,result_rows,result_bytes,memory_usage,ProfileEvents,Settings FROM system.query_log WHERE type='QueryFinish' AND is_initial_query=1 AND (query_id IN ("+','.join("'"+qid+"'" for qid in ids)+") OR (startsWith(query_id,'wildcard-') AND position(query,'"+engine.database+".')>0)) ORDER BY event_time_microseconds"
    for attempt in range(8):
        if attempt:time.sleep(.1)
        engine.query('SYSTEM FLUSH LOGS',logging=False)
        logs=[dict(zip(names,row)) for row in engine.query(sql,logging=False)]
        (output/'query-log.json').write_text(json.dumps(logs,indent=2)+'\n')
        if len(logs)>=len(records):return bind_profiles(records,logs)
    raise ValueError('incomplete query-log cohort after bounded collection')
