#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Repeated adaptive-cache experiments; insertion TTL is never inferred from eviction."""
import argparse
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path
import statistics
import time
import uuid
from fdw_tuning import ROOT, owned, pg_literal, sweeps
from fdw_values import verify_copy
from route_workers import PgWorker
from fdw_adaptive import foreign

HOT_PATTERNS=('evil*','*evil*','*evil.test*','*test/power*','*powershell*',
              '*shell*','*evil.test/powershell','evil.test/power*')
MODES=('capacity','reuse','ttl','settings')


def capacity_summary(records, elapsed):
    if not 0<=elapsed<60:raise ValueError('capacity round overlaps insertion TTL; eviction evidence is confounded')
    hot=[r for r in records if r['phase']=='capacity_hot']
    if len(hot)!=40:raise ValueError('incomplete capacity hot cohort')
    if any(r['cache_hits'] not in (0,1) or r['probes']<0 for r in hot):
        raise ValueError('invalid cache counters')
    return dict(round_seconds=elapsed,hot_hit_rate=sum(r['cache_hits'] for r in hot)/40,
                hot_probes=sum(r['probes'] for r in hot),
                hot_median_ms=statistics.median(r['request_ms'] for r in hot))


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--state',type=Path,required=True)
    parser.add_argument('--pg-container',required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--rounds',type=int,default=5)
    parser.add_argument('--modes',nargs='+',choices=MODES,default=list(MODES))
    args=parser.parse_args()
    if not 1<=args.rounds<=5 or len(args.modes)!=len(set(args.modes)):parser.error('invalid experiment bounds')
    state=json.loads(args.state.read_text());owned(state,args.pg_container)
    out=args.output;out.mkdir(parents=True,exist_ok=False)
    schema='wc_cache_'+uuid.uuid4().hex
    engine=admin=None;workers=[];servers=[];records=[];summaries=[];controls=[]
    def open_worker(label):
        worker=PgWorker(args.pg_container,ROOT,out/(label+'.private.log'))
        workers.append(worker)
        return worker
    def close_worker(worker):
        worker.close();workers.remove(worker)
    def sql(pattern,table='auto1'):
        return f'SELECT id,url,extra FROM {schema}.{table} WHERE wc_match(url,{pg_literal(pattern)},\'raw\')'
    def request(worker,phase,round_no,pattern,table='auto1'):
        row=dict(phase=phase,round=round_no,pattern=pattern,table=table,
                 rows=125 if pattern in HOT_PATTERNS else 0,sql=sql(pattern,table),host_load=list(os.getloadavg()))
        (out/'pending.json').write_text(json.dumps(row)+'\n')
        row.update(worker.request(row['sql'],True))
        scan=foreign(row['plan']['Plan'])
        if (scan is None or scan['Actual Rows']!=row['rows'] or scan['FDW remote rows']!=row['rows']
            or not scan['FDW metrics complete'] or scan.get('Rows Removed by Filter',0)
            or scan['FDW row timing enabled']):raise ValueError('cache benchmark result/completion differs')
        row.update(query_id=scan['FDW last query ID'],selected_sql=scan['ClickHouse SQL'],
            candidate_sql=scan.get('ClickHouse candidate SQL'),cache_hits=scan['FDW plan cache hits'],
            probes=scan['FDW plan probes'],completed_monotonic=time.monotonic(),connection_setup_ms=worker.setup_ms)
        if row['candidate_sql'] is None:raise ValueError('query has no candidate alternative')
        records.append(row)
        with (out/'raw.jsonl').open('a') as f:f.write(json.dumps(row)+'\n')
        (out/'pending.json').unlink()
        print(json.dumps({k:row[k] for k in ('phase','round','cache_hits','probes','request_ms')}),flush=True)
        return row
    try:
        engine=sweeps.Engine(str(args.state),timeout_seconds=60)
        engine.query('CREATE TABLE events(id Int64,url String,extra String) ENGINE=MergeTree ORDER BY id')
        engine.query("INSERT INTO events SELECT number,if(number<125000 AND number%1000=0,'evil.test/powershell','benign.test/normal'),repeat('x',16) FROM numbers(1000000)")
        admin=open_worker('admin');admin.execute(f'CREATE SCHEMA {schema};')
        for threads in (1,2):
            server=f'{schema}_{threads}';servers.append(server)
            admin.execute(f'CREATE SERVER {server} FOREIGN DATA WRAPPER wildcard_fdw OPTIONS('
                f'url {pg_literal(state["url"])},database {pg_literal(engine.database)},timeout_ms \'65000\','
                f'settings \'max_threads={threads},max_block_size=1024,max_execution_time=60\','
                f'terminal_policy \'in_band\',batch_bytes \'16777216\'); '
                f'CREATE USER MAPPING FOR CURRENT_USER SERVER {server} OPTIONS(bearer_token {pg_literal(state["token"])});')
            admin.execute(f'CREATE FOREIGN TABLE {schema}.auto{threads}(id bigint,url text,extra text) SERVER {server} '
                f'OPTIONS(table {pg_literal(engine.database+".events")},candidate_columns \'url\',candidate_mode \'auto\');')
        admin.execute(f'CREATE FOREIGN TABLE {schema}.manual(id bigint,url text,extra text) SERVER {servers[0]} '
                      f'OPTIONS(table {pg_literal(engine.database+".events")});')
        settings=dict(engine=engine.query('SELECT version()')[0][0],machine=sweeps.machine_settings(),
            engine_cpus=state['cpus'],engine_memory_bytes=state['memory_bytes'],rounds=args.rounds,modes=args.modes,
            patterns=HOT_PATTERNS,terminal_policy='in_band',batch_bytes=16777216,rows=1000000,
            scope='backend-local preferences only; native and OS caches uncontrolled; TIMING OFF, overlapping diagnostics',
            extension_sha256=subprocess.check_output(['docker','exec',args.pg_container,'sha256sum',
                '/usr/lib/postgresql/18/lib/pg_wildcard.so'],text=True).split()[0],
            runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
        (out/'settings.json').write_text(json.dumps(settings,indent=2)+'\n')
        # A separate backend with manual exact predicates cannot warm auto caches.
        control=open_worker('exact-controls')
        for pattern in HOT_PATTERNS+tuple(f'*never{i:04}*' for i in range(40)):
            control.execute('COPY ('+sql(pattern,'manual')+') TO STDOUT (FORMAT binary);')
            controls.append(dict(pattern=pattern,value_control=verify_copy(control.file.read_bytes(),'selective',
                125 if pattern in HOT_PATTERNS else 0)))
        close_worker(control)
        (out/'value-controls.json').write_text(json.dumps(controls,indent=2)+'\n')
        if 'capacity' in args.modes:
            for round_no in range(args.rounds):
                worker=open_worker(f'capacity-{round_no}');start=time.monotonic();begin=len(records)
                warm=[request(worker,'capacity_warmup',round_no,p) for p in HOT_PATTERNS]
                if (len({r['candidate_sql'] for r in warm})!=8 or any(r['cache_hits'] or not r['probes'] for r in warm)):
                    raise ValueError('hot patterns do not produce distinct cold keys')
                for i in range(40):
                    cold=request(worker,'capacity_cold',round_no,f'*never{i:04}*')
                    if cold['cache_hits'] or not cold['probes']:raise ValueError('new cold key reused a preference')
                    request(worker,'capacity_hot',round_no,HOT_PATTERNS[i%8])
                summaries.append(dict(round=round_no,**capacity_summary(records[begin:],time.monotonic()-start)))
                close_worker(worker)
        if 'reuse' in args.modes:
            for round_no in range(args.rounds):
                worker=open_worker(f'reuse-{round_no}')
                for step in ('cold','warm1','warm2'):
                    row=request(worker,'reuse_'+step,round_no,HOT_PATTERNS[0])
                    if (row['cache_hits']!=int(step!='cold') or bool(row['probes'])!=(step=='cold')):
                        raise ValueError('reuse cache boundary differs')
                close_worker(worker)
                for step in range(3):
                    worker=open_worker(f'churn-{round_no}-{step}')
                    row=request(worker,'churn',round_no,HOT_PATTERNS[0])
                    if row['cache_hits'] or not row['probes']:raise ValueError('fresh backend was not cold')
                    close_worker(worker)
        if 'settings' in args.modes:
            for round_no in range(args.rounds):
                worker=open_worker(f'settings-{round_no}');cohort=[]
                for i,table in enumerate(('auto1','auto1','auto2','auto1')):
                    row=request(worker,'settings_'+str(i),round_no,HOT_PATTERNS[0],table);cohort.append(row)
                    if row['cache_hits']!=(i in (1,3)):raise ValueError('settings identity cache boundary differs')
                if len({(r['selected_sql'],r['candidate_sql']) for r in cohort})!=1:
                    raise ValueError('settings case changed SQL identity')
                close_worker(worker)
        if 'ttl' in args.modes:
            cohort=[]
            for round_no in range(args.rounds):
                worker=open_worker(f'ttl-{round_no}')
                first=request(worker,'ttl_insert',round_no,HOT_PATTERNS[0]);cohort.append((worker,first))
            for delay,phase in ((30,'ttl_middle'),(61,'ttl_expired')):
                deadline=max(first['completed_monotonic'] for _,first in cohort)+delay
                while time.monotonic()<deadline:time.sleep(min(1,deadline-time.monotonic()))
                for worker,first in cohort:
                    age=time.monotonic()-first['completed_monotonic']
                    row=request(worker,phase,first['round'],HOT_PATTERNS[0])
                    if phase=='ttl_middle' and not (30<=age<59 and row['cache_hits']==1 and row['probes']==0):
                        raise ValueError('intermediate hit crossed TTL or missed')
                    if phase=='ttl_expired' and not (age>=61 and row['cache_hits']==0 and row['probes']>0):
                        raise ValueError('insertion TTL extended or did not expire')
                    row['age_since_insert_completion_seconds']=age
            for worker,_ in cohort:close_worker(worker)
        ids=[r['query_id'] for r in records]
        if len(set(ids))!=len(ids) or any(not re.fullmatch(r'[A-Za-z0-9_-]{1,128}',q) for q in ids):
            raise ValueError('duplicate or invalid measured query IDs')
        engine.query('SYSTEM FLUSH LOGS',logging=False)
        logs=engine.query("SELECT query_id,query_duration_ms,read_rows,result_rows,ProfileEvents,Settings FROM system.query_log WHERE type='QueryFinish' AND is_initial_query=1 AND query_id IN ("+','.join("'"+qid+"'" for qid in ids)+")",logging=False)
        (out/'query-log.json').write_text(json.dumps(logs,indent=2)+'\n')
        byid={r[0]:r for r in logs}
        if len(logs)!=len(ids) or len(byid)!=len(logs):raise ValueError('incomplete or duplicate native cohort')
        for row in records:
            native=byid[row['query_id']]
            if int(native[3])!=row['rows'] or native[5].get('output_format_arrow_compression_method')!='none':
                raise ValueError('native value/compression contract differs')
            row['profile']=dict(server_ms=native[1],read_rows=native[2],result_rows=native[3],events=native[4],settings=native[5])
        (out/'correlated.json').write_text(json.dumps(records,indent=2)+'\n')
        (out/'capacity-summary.json').write_text(json.dumps(summaries,indent=2)+'\n')
    except Exception as error:
        (out/'failure.json').write_text(json.dumps(dict(error=sweeps.redact(str(error),state),completed=len(records)))+'\n');raise
    finally:
        failures=[]
        for worker in list(workers):
            if worker is admin:continue
            try:close_worker(worker)
            except Exception:failures.append('worker cleanup')
        if admin:
            try:
                admin.execute(f'DROP SCHEMA IF EXISTS {schema} CASCADE;')
                for server in servers:admin.execute(f'DROP SERVER IF EXISTS {server} CASCADE;')
            except Exception:failures.append('PostgreSQL cleanup')
            finally:admin.close()
        if engine:
            try:engine.close()
            except Exception:failures.append('native cleanup')
        (out/'cleanup.json').write_text(json.dumps(dict(failures=failures))+'\n')
        if failures:raise RuntimeError('owned fixture cleanup incomplete')

if __name__=='__main__':main()
