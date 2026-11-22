#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Terminal-policy comparison on small results and genuinely indexed Parquet files."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import uuid
import xml.etree.ElementTree as ET
from fdw_tuning import (ROOT, POLICIES, ROUTES, owned, pg_literal, sweeps,
                        schedule, summarize, native_request, verify_arrow)
from fdw_correlation import collect_profiles, selected_sql
from fdw_adaptive import foreign
from fdw_values import verify_copy
from route_workers import DirectWorker, PgWorker


def validate_file_reads(record):
    expected={'lake_indexed':1,'lake_full':8}[record['workload']]
    actual=record['profile']['profile_events'].get('EngineFileLikeReadFiles')
    if actual is None or int(actual)!=expected:
        raise ValueError('native file-read count does not prove expected pruning')
    parquet=record['profile']['profile_events'].get('ParquetRowsFilterExpression')
    if parquet is None or int(parquet)!=expected*125000:
        raise ValueError('native Parquet row work does not match expected file pruning')
    return int(actual)


def index_load_sql(remote):
    # psql meta-command paths are not PostgreSQL E-string literals.
    if any(c in remote for c in "\0\n\r\\'"):
        raise ValueError('index path contains unsupported psql filename characters')
    def filename(name):return "'"+remote+'/'+name+"'"
    # psql reads these private workspace files; the server's OS user need not.
    return ('BEGIN;\n'
        f"\\copy wc_lake_index(index_name,column_name,views,snapshot_id) FROM {filename('index.csv')} CSV\n"
        f"\\copy wc_lake_file FROM {filename('files.csv')} CSV\n"
        f"\\copy wc_lake_gram FROM {filename('grams.csv')} CSV\nCOMMIT;")


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--state',type=Path,required=True)
    parser.add_argument('--pg-container',required=True)
    parser.add_argument('--worker',type=Path,required=True)
    parser.add_argument('--cli',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--repeats',type=int,default=5)
    parser.add_argument('--policies',nargs='+',choices=POLICIES,default=list(POLICIES))
    parser.add_argument('--expected-flush-ms',type=int,choices=(100,1000),default=1000)
    args=parser.parse_args()
    if not 1<=args.repeats<=5 or len(set(args.policies))!=len(args.policies):parser.error('invalid experiment bounds')
    state=json.loads(args.state.read_text());owned(state,args.pg_container)
    config=Path(state['artifact_dir'])/'benchmark.xml'
    if int(ET.parse(config).findtext('query_log/flush_interval_milliseconds'))!=args.expected_flush_ms:
        raise ValueError('fixture configuration differs from requested flush interval')
    out=args.output.resolve();out.mkdir(parents=True,exist_ok=False)
    # PG fixture mounts this repository at /workspace for COPY and worker files.
    out.relative_to(ROOT)
    name='wc_terminal_'+uuid.uuid4().hex
    files=Path(state['share_dir'])/name
    files.mkdir(exist_ok=False)
    remote='/var/lib/clickhouse/user_files/benchmark/'+name
    engine=admin=None;clients={};pg={};servers=[];records=[]
    def emit(record):
        records.append(record)
        with (out/'raw.jsonl').open('a') as f:f.write(json.dumps(record)+'\n')
    try:
        import pyarrow as pa
        import pyarrow.parquet as pq
        for file_no in range(8):
            ids=list(range(file_no*125000,(file_no+1)*125000))
            pq.write_table(pa.table({'id':pa.array(ids,type=pa.int64()),
                'url':['evil.test/powershell' if file_no==0 and i%1000==0 else 'benign.test/normal' for i in ids],
                'extra':['x'*16]*len(ids)}),files/f'f{file_no}.parquet',compression='snappy')
        with (out/'index.log').open('w') as log:
            subprocess.run([str(args.cli.resolve()),'index','--column','url','--views','raw','--snapshot-id','1',
                '--out',str(out/'lake.puffin'),*[str(files/f'f{i}.parquet')+'='+remote+f'/f{i}.parquet' for i in range(8)]],
                check=True,stdout=log,stderr=subprocess.STDOUT,timeout=300)
            subprocess.run([str(args.cli.resolve()),'export-pg',str(out/'lake.puffin'),'--name',name,
                '--out-dir',str(out/'index')],check=True,stdout=log,stderr=subprocess.STDOUT,timeout=60)
        engine=sweeps.Engine(str(args.state),timeout_seconds=60)
        engine.query('CREATE TABLE small(id Int64,url String,extra String) ENGINE=MergeTree ORDER BY id')
        engine.query("INSERT INTO small SELECT number*1000,'evil.test/powershell',repeat('x',16) FROM numbers(125)")
        engine.query(f"CREATE VIEW lake_source AS SELECT id,url,extra,_path FROM file('benchmark/{name}/*.parquet',Parquet)")
        paths=[row[0] for row in engine.query('SELECT DISTINCT _path FROM lake_source ORDER BY _path')]
        if paths!=[remote+f'/f{i}.parquet' for i in range(8)]:raise ValueError('native _path differs from genuine index mapping')
        admin=PgWorker(args.pg_container,ROOT,out/'admin.private.log')
        admin.execute(f'CREATE SCHEMA {name};')
        index_remote='/workspace/'+str((out/'index').relative_to(ROOT))
        admin.execute(index_load_sql(index_remote))
        for policy in args.policies:
            server=f'{name}_{policy}';servers.append(server)
            admin.execute(f'CREATE SERVER {server} FOREIGN DATA WRAPPER wildcard_fdw OPTIONS('
                f'url {pg_literal(state["url"])},database {pg_literal(engine.database)},timeout_ms \'65000\','
                f'settings \'max_threads=2,max_block_size=1024,max_execution_time=60\','
                f'terminal_policy {pg_literal(policy)},batch_bytes \'16777216\'); '
                f'CREATE USER MAPPING FOR CURRENT_USER SERVER {server} OPTIONS(bearer_token {pg_literal(state["token"])});')
            for workload in ('small','lake_indexed','lake_full'):
                table='small' if workload=='small' else 'lake_source'
                extra=',lake_index '+pg_literal(name) if workload=='lake_indexed' else ''
                admin.execute(f'CREATE FOREIGN TABLE {name}.{workload}_{policy}(id bigint,url text,extra text) '
                    f'SERVER {server} OPTIONS(table {pg_literal(engine.database+"."+table)}{extra});')
            pg[policy]=PgWorker(args.pg_container,ROOT,out/f'pg-{policy}.private.log')
            clients[policy]=DirectWorker(args.worker.resolve(),state,out/f'direct-{policy}.private.log',
                extra_args=[policy,'16777216'])
        sqls={};native={};explain={}
        for workload in ('small','lake_indexed','lake_full'):
            for policy in args.policies:
                sqls[workload,policy]=f"SELECT id,url,extra FROM {name}.{workload}_{policy} WHERE wc_match(url,'*powershell*','raw')"
                admin.execute('EXPLAIN(VERBOSE,FORMAT JSON) '+sqls[workload,policy]+';')
                plan=json.loads(admin.file.read_bytes())[0]
                scan=foreign(plan['Plan']);native[workload,policy]=scan['ClickHouse SQL']
                explain[workload,policy]=plan
                if workload=='lake_indexed' and 'NOT IN' not in scan['ClickHouse SQL']:
                    raise ValueError('indexed PG query has no conservative exclusion predicate')
        for workload in ('small','lake_indexed','lake_full'):
            if len({native[workload,p] for p in args.policies})!=1:raise ValueError('terminal policies changed selected SQL')
        settings=dict(engine=engine.query('SELECT version()')[0][0],machine=sweeps.machine_settings(),
            engine_cpus=state['cpus'],engine_memory_bytes=state['memory_bytes'],repeats=args.repeats,
            policies=args.policies,query_log_flush_interval_ms=args.expected_flush_ms,
            pg_cache_policy='inherited; inspect ProfileEvents; native/direct disable result and condition caches',
            source_files=8,source_rows=1000000,selected_rows=125,batch_bytes=16777216,block_size=1024,
            source_kind='eight Parquet files via passthrough native view; genuine wc-lake Puffin import; not Iceberg metadata or delete-file timing',
            terminal_clocks='published-client aggregate per-operation counters; not per-poll clocks; overlap with data/server/PG clocks',
            log_collection='after entire cohort; no diagnostic flush between requests; policy-owned flushes are part of the measured operation',
            runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            worker_sha256=hashlib.sha256(args.worker.read_bytes()).hexdigest(),
            cli_sha256=hashlib.sha256(args.cli.read_bytes()).hexdigest(),
            extension_sha256=subprocess.check_output(['docker','exec',args.pg_container,'sha256sum',
                '/usr/lib/postgresql/18/lib/pg_wildcard.so'],text=True).split()[0],
            native_paths=paths,parquet_sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in files.iterdir()})
        (out/'settings.json').write_text(json.dumps(settings,indent=2)+'\n')
        (out/'explain.json').write_text(json.dumps({w+':'+p:plan for (w,p),plan in explain.items()},indent=2)+'\n')
        sources=out/'sources';sources.mkdir()
        for path in [Path(__file__),ROOT/'tests/benchmarks/fdw_tuning.py',ROOT/'tests/benchmarks/fdw_correlation.py',
                     ROOT/'tests/benchmarks/fdw_values.py',ROOT/'tests/benchmarks/route_workers.py',ROOT/'tests/transport/examples/fdw_warm.rs']:
            (sources/path.name).write_bytes(path.read_bytes())
        for workload in ('small','lake_indexed','lake_full'):
            for repeat,policy,block,route in schedule([(p,1024) for p in args.policies],ROUTES,args.repeats):
                sql=sqls[workload,policy];remote_sql=native[workload,policy]
                record=dict(workload=workload,repeat=repeat,policy=policy,block_size=block,route=route,
                    status='pending',rows=125,pg_sql=sql,native_sql=remote_sql,host_load=list(os.getloadavg()))
                (out/'pending.json').write_text(json.dumps(record)+'\n')
                if route=='native':
                    result,raw=native_request(engine,remote_sql,block)
                    record.update(result,value_control=verify_arrow(raw,'selective',125))
                elif route=='direct':
                    worker=clients[policy]
                    worker.process.stdin.write((json.dumps(dict(sql=remote_sql,workload='selective',rows=125,
                        verify=repeat<0,block_size=block))+'\n').encode());worker.process.stdin.flush()
                    result=worker.read(75)
                    record.update(result,workload=workload,route=route)
                    if 'error' in result:
                        record['status']='error';emit(record);raise RuntimeError('direct operation failed; typed evidence retained')
                elif route=='pg_backend':
                    record.update(pg[policy].request(sql,True))
                    scan=foreign(record['plan']['Plan'])
                    if (scan['Actual Rows']!=125 or scan['FDW remote rows']!=125 or not scan['FDW metrics complete']
                        or scan.get('Rows Removed by Filter',0) or scan['FDW row timing enabled']):
                        raise ValueError('PostgreSQL count/completion/recheck differs')
                    if scan['ClickHouse SQL']!=remote_sql:raise ValueError('analyzed SQL changed from native/direct control')
                    record.update(query_id=scan['FDW last query ID'],selected_sql=scan['ClickHouse SQL'])
                else:
                    worker=pg[policy]
                    record['request_ms']=worker.execute('COPY ('+sql+') TO STDOUT (FORMAT binary);')
                    record['value_control']=verify_copy(worker.file.read_bytes(),'selective',125)
                record['status']='success';emit(record);(out/'pending.json').unlink()
                print(json.dumps({k:record[k] for k in ('workload','repeat','policy','route','request_ms')}),flush=True)
        records=collect_profiles(engine,records,out)
        query_text={row['query_id']:row['query'] for row in json.loads((out/'query-log.json').read_text())}
        for record in records:
            if selected_sql(query_text[record['query_id']])!=selected_sql(record['native_sql']):
                raise ValueError('native log SQL differs from generated control')
            if record['workload'].startswith('lake_'):validate_file_reads(record)
        (out/'correlated.json').write_text(json.dumps(records,indent=2)+'\n')
        (out/'summary.json').write_text(json.dumps(summarize(records),indent=2)+'\n')
    except Exception as error:
        (out/'failure.json').write_text(json.dumps(dict(error=sweeps.redact(str(error),state),completed=len(records)))+'\n');raise
    finally:
        failures=[]
        for worker in list(pg.values())+list(clients.values()):
            try:worker.close()
            except Exception:failures.append('worker cleanup')
        if admin:
            admin.close()
            admin=None
            try:
                # ON_ERROR_STOP can terminate the setup session; cleanup must
                # not depend on that failed connection remaining usable.
                admin=PgWorker(args.pg_container,ROOT,out/'cleanup.private.log')
                admin.execute(f'DROP SCHEMA IF EXISTS {name} CASCADE; DELETE FROM wc_lake_index WHERE index_name={pg_literal(name)};')
                for server in servers:admin.execute(f'DROP SERVER IF EXISTS {server} CASCADE;')
            except Exception:failures.append('PG cleanup')
            finally:
                if admin:admin.close()
        if engine:
            try:engine.close()
            except Exception:failures.append('native cleanup')
        try:shutil.rmtree(files)
        except Exception:failures.append('owned Parquet files cleanup')
        (out/'cleanup.json').write_text(json.dumps(dict(failures=failures))+'\n')
        if failures:raise RuntimeError('owned cleanup incomplete')

if __name__=='__main__':main()
