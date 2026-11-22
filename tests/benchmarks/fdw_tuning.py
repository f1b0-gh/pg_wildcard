#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Repeated native/direct/FDW streaming controls on explicitly owned fixtures.

Run with a Python environment containing PyArrow. No build or test should run
alongside measurement. Private fixture states and diagnostics stay in .local/.
"""
import argparse
from collections import defaultdict
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import time
import xml.etree.ElementTree as ET
import urllib.parse
import urllib.request
import uuid

from compare_routes import ROOT, foreign_nodes, sweeps
from fdw_profile import pg_literal
from fdw_values import expected_row, verify_copy
from fdw_correlation import collect_profiles, selected_sql
from route_workers import DirectWorker, PgWorker

ROUTES=('native','direct','pg_backend','pg_copy')
POLICIES=('flush_first','observe_first','observe_only','in_band')


def schedule(configs, routes, repeats):
    cells=[(policy,block,route) for policy,block in configs for route in routes]
    for repeat in range(-1,repeats):
        offset=(max(repeat,0)*5)%len(cells)
        for cell in cells[offset:]+cells[:offset]:yield (repeat,*cell)


def summarize(records):
    groups=defaultdict(list)
    for record in records:
        if record['repeat']<0:continue
        if record['status'] not in ('success','resource_refusal'):
            raise ValueError('failed measurement cannot be summarized as success')
        if record['status']=='success' and ('request_ms' not in record or record['request_ms']<0):
            raise ValueError('successful measurement requires elapsed time')
        groups[tuple(record[k] for k in ('workload','policy','block_size','route'))].append(record)
    result=[]
    for key,group in groups.items():
        values=[r['request_ms'] for r in group if r['status']=='success']
        result.append(dict(zip(('workload','policy','block_size','route'),key),
            samples=len(values),refusals=sum(r['status']=='resource_refusal' for r in group),
            median_ms=statistics.median(values) if values else None,
            min_ms=min(values) if values else None,max_ms=max(values) if values else None))
    return result


def owned(state, pg_container):
    for name,label,value in [(state['name'],'wildcard.benchmark.owner',state['owner']),
                             (pg_container,'pg_wildcard.task','review-20261122')]:
        info=json.loads(subprocess.check_output(['docker','inspect',name],text=True))[0]
        if info['Config']['Labels'].get(label)!=value or not info['State']['Running']:
            raise ValueError('fixture must be running and explicitly owned')


def verify_arrow(raw, workload, rows):
    import pyarrow as pa
    import struct
    table=pa.ipc.open_stream(raw).read_all()
    if table.num_rows!=rows or table.num_columns!=(1 if workload=='narrow' else 3):
        raise ValueError('native Arrow shape differs')
    seen=bytearray(rows)
    for batch in table.to_batches():
        columns=[column.to_pylist() for column in batch.columns]
        for i,identifier in enumerate(columns[0]):
            stride=1000 if workload=='selective' else 1
            if not isinstance(identifier,int) or identifier%stride or not 0<=identifier//stride<rows:
                raise ValueError('native ID outside oracle')
            ordinal=identifier//stride
            if seen[ordinal]:raise ValueError('duplicate native ID')
            values=(struct.pack('!q',identifier),)+tuple(
                c[i].encode() if isinstance(c[i],str) else c[i] for c in columns[1:])
            if values!=expected_row(workload,identifier):raise ValueError('native value mismatch')
            seen[ordinal]=1
    return {'rows':rows,'oracle':'every field and complete unique row-ID set'}


def native_request(engine, sql, block):
    qid='fdw-native-'+uuid.uuid4().hex
    params=dict(database=engine.database,default_format='ArrowStream',query_id=qid,
        max_threads=2,max_block_size=block,max_result_rows=1000000,
        max_result_bytes=67108864,result_overflow_mode='throw',max_execution_time=60,
        max_memory_usage=4000000000,use_query_cache=0,use_query_condition_cache=0,
        output_format_arrow_compression_method='none',enable_http_compression=0,
        output_format_arrow_string_as_string=0,output_format_arrow_low_cardinality_as_dictionary=0,
        session_timezone='UTC',cancel_http_readonly_queries_on_client_close=1)
    request=urllib.request.Request(engine.state['url']+'/?'+urllib.parse.urlencode(params),
        data=sql.encode(),headers={'Authorization':'Bearer '+engine.state['token']})
    start=time.monotonic()
    with engine.opener.open(request,timeout=65) as response:raw=response.read(67108865)
    elapsed=(time.monotonic()-start)*1000
    if len(raw)>67108864:raise ValueError('native response byte limit')
    return {'query_id':qid,'request_ms':elapsed,'http_body_bytes':len(raw)},raw


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--state',type=Path,required=True)
    p.add_argument('--pg-container',required=True)
    p.add_argument('--worker',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--blocks',default='1024,4096,16384')
    p.add_argument('--policies',default='in_band')
    p.add_argument('--workloads',default='narrow,smalltext,wide1k')
    p.add_argument('--repeats',type=int,default=5)
    p.add_argument('--batch-bytes',type=int,default=16777216)
    p.add_argument('--rows',type=int,default=1000000)
    p.add_argument('--routes',nargs='+',choices=ROUTES,default=list(ROUTES))
    a=p.parse_args()
    blocks=[int(x) for x in a.blocks.split(',')];policies=a.policies.split(',')
    workloads=a.workloads.split(',')
    if (not blocks or any(n not in (1024,4096,16384) for n in blocks)
        or len(set(blocks))!=len(blocks) or len(set(policies))!=len(policies)
        or not policies or set(policies)-set(POLICIES)
        or not workloads or set(workloads)-{'narrow','smalltext','wide1k','broad_recheck'}
        or not 1<=a.repeats<=10 or not 1<=a.rows<=1000000
        or len(set(a.routes))!=len(a.routes)
        or ('pg_copy' in a.routes and 'pg_backend' not in a.routes)
        or a.batch_bytes not in (1048576,4194304,16777216)):
        p.error('unsupported experiment bounds')
    state=json.loads(a.state.read_text());owned(state,a.pg_container)
    a.output.mkdir(parents=True,exist_ok=False)
    engine=admin=None;clients={};pg={};records=[];servers=[]
    schema='wc_tuning_'+uuid.uuid4().hex
    def emit(record):
        records.append(record)
        with (a.output/'raw.jsonl').open('a') as f:f.write(json.dumps(record)+'\n')
    try:
        engine=sweeps.Engine(str(a.state),timeout_seconds=60)
        engine.query('CREATE TABLE events(id Int64,url String,extra String) ENGINE=MergeTree ORDER BY id')
        engine.query("INSERT INTO events SELECT number,if(number%10000=0,'evil.test/powershell','benign.test/normal'),repeat('x',16) FROM numbers("+str(a.rows)+")")
        wide_rows=min(a.rows,32768)
        engine.query('CREATE TABLE wide_events AS events')
        engine.query("INSERT INTO wide_events SELECT number,'benign.test/normal',repeat('x',1024) FROM numbers("+str(wide_rows)+")")
        admin=PgWorker(a.pg_container,ROOT,a.output/'admin.private.log')
        admin.execute(f'CREATE SCHEMA {schema};')
        configs=[(policy,block) for policy in policies for block in blocks]
        for policy,block in configs:
            name=f'{schema}_{policy}_{block}'
            admin.execute(f'CREATE SERVER {name} FOREIGN DATA WRAPPER wildcard_fdw OPTIONS('
                f'url {pg_literal(state["url"])},database {pg_literal(engine.database)},timeout_ms \'65000\','
                f'settings \'max_threads=2,max_block_size={block},max_execution_time=60\','
                f'terminal_policy {pg_literal(policy)},batch_bytes {pg_literal(str(a.batch_bytes))}); '
                f'CREATE USER MAPPING FOR CURRENT_USER SERVER {name} OPTIONS(bearer_token {pg_literal(state["token"])});')
            servers.append(name)
            for table in ('events','wide_events'):
                admin.execute(f'CREATE FOREIGN TABLE {schema}.{table}_{policy}_{block}(id bigint,url text,extra text) '
                    f'SERVER {name} OPTIONS(table {pg_literal(engine.database+"."+table)});')
            pg[policy,block]=PgWorker(a.pg_container,ROOT,a.output/f'pg-{policy}-{block}.private.log')
            clients[policy,block]=DirectWorker(a.worker.resolve(),state,a.output/f'direct-{policy}-{block}.private.log',
                extra_args=[policy,str(a.batch_bytes)])
        settings=dict(engine=engine.query('SELECT version()')[0][0],machine=sweeps.machine_settings(),
            engine_cpus=state['cpus'],engine_memory_bytes=state['memory_bytes'],
            rows=a.rows,wide_rows=wide_rows,configs=configs,workloads=workloads,repeats=a.repeats,routes=a.routes,
            batch_bytes=a.batch_bytes,host_load=list(os.getloadavg()),
            runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            worker_sha256=hashlib.sha256(a.worker.read_bytes()).hexdigest(),
            pg_cache_policy='inherited; inspect ProfileEvents',
            clocks='warm persistent clients, full native Arrow body / all confirmed direct batches / backend TIMING OFF / full COPY; native/COPY verification and query-log collection outside clocks; direct verification inside excluded warmups; overlapping diagnostics',
            log_collection='after all measurement requests; no diagnostic SYSTEM FLUSH between samples')
        admin.execute("SELECT current_setting('server_version');")
        settings['postgres']=admin.file.read_text().strip()
        if not settings['postgres']:raise ValueError('PostgreSQL version query returned no value')
        settings['native_arrow_compression']='none'
        settings['extension_sha256']=subprocess.check_output(['docker','exec',a.pg_container,
            'sha256sum','/usr/lib/postgresql/18/lib/pg_wildcard.so'],text=True).split()[0]
        config=Path(state['artifact_dir'])/'benchmark.xml'
        settings['query_log_flush_interval_ms']=int(ET.parse(config).findtext('query_log/flush_interval_milliseconds'))
        sources=a.output/'sources';sources.mkdir()
        for path in [Path(__file__),ROOT/'tests/benchmarks/fdw_values.py',
                     ROOT/'tests/benchmarks/fdw_correlation.py',
                     ROOT/'tests/benchmarks/route_workers.py',ROOT/'libwildcard/tests/benchmarks/sweeps.py',
                     ROOT/'tests/transport/examples/fdw_warm.rs']:
            (sources/path.name).write_bytes(path.read_bytes())
        (a.output/'settings.json').write_text(json.dumps(settings,indent=2)+'\n')
        for workload in workloads:
            table='wide_events' if workload=='wide1k' else 'events'
            rows=wide_rows if workload=='wide1k' else a.rows
            projection='id' if workload=='narrow' else 'id,url,extra'
            predicate=" WHERE url LIKE '%normal%' OR url LIKE '%powershell%'" if workload=='broad_recheck' else ''
            native=f'SELECT {projection} FROM {engine.database}.{table}'+predicate
            for repeat,policy,block,route in schedule(configs,a.routes,a.repeats):
                sql=f'SELECT {projection} FROM {schema}.{table}_{policy}_{block}'
                if workload=='broad_recheck':sql+=" WHERE wc_match(url,'*normal*','raw') OR wc_match(url,'*powershell*','raw')"
                record=dict(workload=workload,repeat=repeat,policy=policy,block_size=block,
                    route=route,status='pending',rows=rows,native_sql=native,pg_sql=sql,host_load=list(os.getloadavg()))
                (a.output/'pending.json').write_text(json.dumps(record)+'\n')
                if route=='native':
                    result,raw=native_request(engine,native,block)
                    record.update(result,value_control=verify_arrow(raw,workload,rows))
                elif route=='direct':
                    worker=clients[policy,block]
                    command=dict(sql=native,workload=workload,rows=rows,verify=repeat<0,block_size=block)
                    start=time.monotonic()
                    worker.process.stdin.write((json.dumps(command)+'\n').encode());worker.process.stdin.flush()
                    result=worker.read(75)
                    if 'error' in result:
                        record.update(result,status='resource_refusal' if result.get('error_kind')=='LimitExceeded' else 'error')
                        emit(record)
                        raise RuntimeError('direct measurement failed; typed result and private log retained')
                    record.update(result,worker_roundtrip_ms=(time.monotonic()-start)*1000)
                    record['route']=route
                elif route=='pg_backend':
                    record.update(pg[policy,block].request(sql,True))
                    nodes=list(foreign_nodes(record['plan']['Plan']))
                    if len(nodes)!=1 or nodes[0]['Actual Rows']!=rows or not nodes[0]['FDW metrics complete']:
                        raise ValueError('backend result/completion mismatch')
                    record.update(query_id=nodes[0]['FDW last query ID'],selected_sql=nodes[0]['ClickHouse SQL'])
                else:
                    worker=pg[policy,block]
                    record['request_ms']=worker.execute('COPY ('+sql+') TO STDOUT (FORMAT binary);')
                    record['value_control']=verify_copy(worker.file.read_bytes(),workload,rows)
                record['status']='success';emit(record)
                (a.output/'pending.json').unlink()
                print(json.dumps({k:record[k] for k in ('workload','repeat','policy','block_size','route','request_ms')}),flush=True)
        records=collect_profiles(engine,records,a.output)
        (a.output/'correlated.json').write_text(json.dumps(records,indent=2)+'\n')
        (a.output/'summary.json').write_text(json.dumps(summarize(records),indent=2)+'\n')
    except Exception as error:
        (a.output/'failure.json').write_text(json.dumps({
            'error':sweeps.redact(str(error),state),
            'completed_requests':len(records),
            'pending_request_retained':(a.output/'pending.json').exists()})+'\n')
        raise
    finally:
        failures=[]
        for worker in list(clients.values())+list(pg.values()):
            try:worker.close()
            except Exception:failures.append('worker cleanup')
        if admin:
            try:
                admin.execute(f'DROP SCHEMA IF EXISTS {schema} CASCADE;')
                for name in servers:
                    admin.execute(f'DROP SERVER IF EXISTS {name} CASCADE;')
            except Exception:failures.append('PG objects cleanup')
            finally:admin.close()
        if engine:
            try:engine.close()
            except Exception:failures.append('native database cleanup')
        (a.output/'cleanup.json').write_text(json.dumps({'failures':failures})+'\n')
        if failures:raise RuntimeError('owned object cleanup incomplete')


if __name__=='__main__':main()
