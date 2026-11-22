#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Paired published antalya-rs / PostgreSQL FDW native-engine profiler.

Needs an already owned PG container, a native engine, and the release direct
worker. Never starts/stops containers or mutates server-wide settings.
Credentials enter via environment and are never written to results.
"""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import subprocess
import signal
import time
import threading
import uuid
from process_sampler import SamplerProcess, status
import urllib.request


POLICIES = ('flush_first', 'observe_first', 'observe_only', 'in_band')
WORKLOADS = ('narrow', 'wide', 'wide1k', 'broad_recheck', 'selective_indexed', 'selective_full')


def pg_literal(value):
    if '\0' in value:
        raise ValueError('PostgreSQL option value contains NUL')
    return "E'" + value.replace('\\', '\\\\').replace("'", "''") + "'"


def csv_selection(allowed):
    def parse(value):
        if not all(item in allowed for item in value.split(',')):
            raise argparse.ArgumentTypeError('choose comma-separated values from: ' + ','.join(allowed))
        return value
    return parse


def redact(message):
    private = {os.environ.get(key, '') for key in
               ('ANTALYA_TEST_TOKEN', 'ANTALYA_TEST_URL', 'FDW_ENGINE_URL')}
    for value in sorted(private, key=len, reverse=True):
        if value:
            # PostgreSQL errors can echo the SQL-escaped spelling of an option.
            escaped=value.replace('\\', '\\\\').replace("'", "''")
            message=message.replace("E'"+escaped+"'", '[redacted]')
            message=message.replace(escaped, '[redacted]')
            message = message.replace(value, '[redacted]')
    return message


def bounded_process(command, *, data, timeout, sample=False, cancel=None):
    """Own a process group and reap it and its sampler on every exit path."""
    process=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE,text=True,start_new_session=True)
    stopped=threading.Event();peak={};observer=None
    def observe():
        while not stopped.is_set() and process.poll() is None:
            row=status(process.pid)
            if row:
                for key in ('rss_kib','hwm_kib','threads'):
                    peak[key]=max(peak.get(key,0),row[key])
            stopped.wait(.01)
    try:
        if sample:
            observer=threading.Thread(target=observe,name=f'fdw-profile-observer-{process.pid}',daemon=True)
            observer.start()
        deadline=time.monotonic()+timeout
        while True:
            if cancel is not None and cancel.is_set():raise TimeoutError('profiling cancelled')
            remaining=deadline-time.monotonic()
            if remaining<=0:raise TimeoutError('profile worker deadline')
            try:
                stdout,stderr=process.communicate(data,timeout=min(.25,remaining))
                break
            except subprocess.TimeoutExpired:data=None
        if process.returncode:raise RuntimeError(redact(stderr))
        return stdout,peak
    finally:
        try:
            if process.poll() is None:
                try:os.killpg(process.pid,signal.SIGTERM)
                except ProcessLookupError:pass
                try:process.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    try:os.killpg(process.pid,signal.SIGKILL)
                    except ProcessLookupError:pass
                    process.communicate(timeout=5)
        finally:
            stopped.set()
            if observer:
                observer.join(timeout=2)
                if observer.is_alive():raise RuntimeError('profile observer cleanup failed')


def terminate_pg_session(container, tag):
    """Cancel only the uniquely named session started by this invocation."""
    predicate='application_name='+pg_literal(tag)+' AND pid<>pg_backend_pid()'
    sql='SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE '+predicate+'; SELECT count(*) FROM pg_stat_activity WHERE '+predicate+';'
    for _ in range(4):
        result=subprocess.run(['docker','exec','-i','-e','PGOPTIONS=-c statement_timeout=5000 -c lock_timeout=2000',
            container,'timeout','--signal=TERM','--kill-after=2s','8s',
            'psql','-U','postgres','-XAtq','-v','ON_ERROR_STOP=1'],
            input=sql,text=True,capture_output=True,timeout=10)
        if result.returncode:raise RuntimeError('PG session cleanup failed; stop profiling')
        if result.stdout.strip().splitlines()[-1:] == ['0']:return
        time.sleep(.1)
    raise RuntimeError('PG session cleanup unconfirmed; stop profiling')


def pg_request(container, sql, *, timeout=315, statement_ms=300000, lock_ms=10000, cancel=None):
    tag='wc_profile_'+uuid.uuid4().hex
    command=['docker','exec','-i','-e','PGAPPNAME='+tag,
             '-e',f'PGOPTIONS=-c statement_timeout={statement_ms} -c lock_timeout={lock_ms}',
             container,'timeout','--signal=TERM','--kill-after=5s',f'{max(1,timeout-5)}s',
             'psql','-U','postgres','-XAtq','-v','ON_ERROR_STOP=1']
    try:
        stdout,_=bounded_process(command,data=sql,timeout=timeout,cancel=cancel)
        return stdout.strip()
    except BaseException:
        terminate_pg_session(container,tag)
        raise


def persist_cohort(out, cohort, flush_logs, querylog, expected):
    """Preserve measured results before any untimed control request can fail."""
    pending = out / ('pending-cohort-' + uuid.uuid4().hex + '.json')
    pending.write_text(json.dumps({'status':'awaiting_controls','records':cohort}, indent=2)+'\n')
    control_error = None
    try:
        flush_logs()
        for record in cohort:
            if record.get('query_id'):
                record['query_log'] = querylog(record['query_id'])
    except Exception as error:
        control_error = redact(str(error))
        for record in cohort:
            record['control_error'] = control_error
            record.setdefault('error', 'post-cohort control failed: ' + control_error)
    for record in cohort:
        if record.get('query_id'):
            actual=record.get('rows',record.get('scan',{}).get('FDW remote rows'))
            if actual!=expected:
                record.setdefault('error', f'row count {actual}, expected {expected}')
            if 'query_log' in record and len(record['query_log'])!=1:
                record.setdefault('error', 'expected one genuine QueryFinish record')
    # Closing the file flushes every final record before pending evidence is
    # removed. On a write failure the pending artifact is deliberately retained.
    with (out/'raw.jsonl').open('a') as output:
        for record in cohort:
            output.write(json.dumps(record)+'\n')
    pending.unlink()
    if control_error is not None:
        raise RuntimeError('post-cohort control failed; measured records retained in raw.jsonl: '+control_error)
    if any('error' in record for record in cohort):
        # Process cleanup does not prove remote terminal completion. Never start
        # another cohort after a failed/unknown operation; retain its evidence.
        raise RuntimeError('failed cohort retained in raw.jsonl; stop profiling before further work')


def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--output',required=True);ap.add_argument('--pg-container',required=True)
    ap.add_argument('--direct-worker',required=True);ap.add_argument('--setup',action='store_true');ap.add_argument('--setup-only',action='store_true')
    ap.add_argument('--timing-off',action='store_true');ap.add_argument('--concurrency',default='1,8,32');ap.add_argument('--policies',type=csv_selection(POLICIES),default='flush_first,observe_first,observe_only,in_band')
    ap.add_argument('--budgets',default='1048576,16777216');ap.add_argument('--workloads',type=csv_selection(WORKLOADS),default='narrow,wide,selective_indexed,selective_full')
    args=ap.parse_args(); out=Path(args.output);out.mkdir(parents=True,exist_ok=True)
    token=os.environ['ANTALYA_TEST_TOKEN'];url=os.environ['ANTALYA_TEST_URL']
    pgurl=os.environ.get('FDW_ENGINE_URL',url)
    cancel=threading.Event()
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    def ch(sql):
        req=urllib.request.Request(url,data=sql.encode(),headers={'Authorization':'Bearer '+token})
        with opener.open(req,timeout=300) as r:return r.read().decode()
    def pg(sql):
        return pg_request(args.pg_container,sql,cancel=cancel)
    if args.setup:
        ch('CREATE DATABASE IF NOT EXISTS fdw_profile')
        ch('CREATE TABLE IF NOT EXISTS fdw_profile.events(id Int64,url String,extra String) ENGINE=MergeTree ORDER BY id')
        if int(ch('SELECT count() FROM fdw_profile.events'))==0:
            ch("INSERT INTO fdw_profile.events SELECT number,if(number%10000=0,'evil.test/powershell','benign.test/normal'),repeat('x',16) FROM numbers(1000000)")
        ch('CREATE TABLE IF NOT EXISTS fdw_profile.wide_events(id Int64,url String,extra String) ENGINE=MergeTree ORDER BY id')
        if int(ch('SELECT count() FROM fdw_profile.wide_events'))==0:
            ch("INSERT INTO fdw_profile.wide_events SELECT number,'benign.test/normal',repeat('x',1024) FROM numbers(32768)")
        pg('CREATE EXTENSION IF NOT EXISTS pg_wildcard; DROP SERVER IF EXISTS fdw_profile CASCADE;')
        pg("CREATE SERVER fdw_profile FOREIGN DATA WRAPPER wildcard_fdw OPTIONS (url "+pg_literal(pgurl)+",database 'fdw_profile',timeout_ms '300000',settings 'max_threads=2',terminal_policy 'flush_first',batch_bytes '16777216'); CREATE USER MAPPING FOR CURRENT_USER SERVER fdw_profile OPTIONS (bearer_token "+pg_literal(token)+"); CREATE FOREIGN TABLE fdw_profile_events(id bigint,url text,extra text) SERVER fdw_profile OPTIONS(table 'fdw_profile.events');")
        pg("CREATE FOREIGN TABLE fdw_profile_wide(id bigint,url text,extra text) SERVER fdw_profile OPTIONS(table 'fdw_profile.wide_events')")
    if args.setup_only:return
    workloads={
        'narrow':'SELECT id FROM fdw_profile_events',
        'wide':'SELECT id,url,extra FROM fdw_profile_events',
        'wide1k':'SELECT id,url,extra FROM fdw_profile_wide',
        'broad_recheck':"SELECT id,url,extra FROM fdw_profile_events WHERE wc_match(url,'*normal*','raw') OR wc_match(url,'*powershell*','raw')",
        'selective_indexed':"SELECT id,url FROM fdw_profile_lake_indexed WHERE wc_match(url,'*powershell*','raw')",
        'selective_full':"SELECT id,url FROM fdw_profile_lake_full WHERE wc_match(url,'*powershell*','raw')",
    }
    def foreign(plan):
        if plan['Node Type']=='Foreign Scan':return plan
        for p in plan.get('Plans',[]):
            f=foreign(p)
            if f:return f
    def querylog(qid):
        # Flush happens after every timed cohort and is not part of either route.
        return json.loads(ch("SELECT query_id,query_duration_ms,read_rows,read_bytes,result_rows,result_bytes,memory_usage,ProfileEvents FROM system.query_log WHERE type='QueryFinish' AND query_id='"+qid+"' FORMAT JSON"))['data']
    settings={'engine':ch('SELECT version()').strip(),'postgres':pg('SELECT version()'),'client':'crates.io antalya-rs =0.1.0','sampler_period_ms':10,'build_profile':'PostgreSQL release opt-level=3 fat LTO codegen-units=1; direct cargo release','cache_policy':'No cache flush; fixture loading and correctness checks precede timing. Direct cohort runs before PostgreSQL cohort. No randomized-order or cold-cache claim.','ttfb':'unavailable: published API exposes first decoded batch, not first HTTP byte','clock_contract':'Nested/overlapping clocks; never sum or subtract to invent exclusive phases. PG Execution Time includes executor timing overhead. Local recheck time is not instrumented separately.','batch_contract':'max_batch_bytes bounds each charged retained batch and its pre-read reservation; client shared retained budget remains 64MiB; no bound on backend RSS. Peak individual charge is not concurrent shared peak.','arguments':vars(args)}
    (out/'settings.json').write_text(json.dumps(settings,indent=2)+'\n')
    pg_peaks={}
    marker=Path('.local')/('fdw-sampler-'+uuid.uuid4().hex)
    sampler_command=['docker','exec',args.pg_container,'python3','/workspace/tests/benchmarks/process_sampler.py','/workspace/'+str(marker)]
    with SamplerProcess(sampler_command, marker) as sampler:
        def samples():
            for line in sampler.stdout:
                try:
                    rows=json.loads(line)
                except ValueError:
                    continue  # A stopped nonblocking writer may leave a partial final line.
                for row in rows:
                    previous=pg_peaks.setdefault(row['pid'],{'rss_kib':0,'hwm_kib':0,'threads':0})
                    for key in previous:previous[key]=max(previous[key],row[key])
        sampler_reader=threading.Thread(target=samples,daemon=True)
        sampler_reader.start()
        records=[]
        for workload in args.workloads.split(','):
          sql=workloads[workload]
          for policy in args.policies.split(','):
           for budget in map(int,args.budgets.split(',')):
            pg(f"ALTER SERVER fdw_profile OPTIONS (SET terminal_policy '{policy}',SET batch_bytes '{budget}')")
            remote=foreign(json.loads(pg('EXPLAIN(FORMAT JSON) '+sql))[0]['Plan'])['ClickHouse SQL']
            for concurrency in map(int,args.concurrency.split(',')):
             for route in ['direct','postgres']:
              def run(i):
                start=time.monotonic()
                record={'workload':workload,'policy':policy,'batch_bytes':budget,'concurrency':concurrency,'worker':i,'route':route,'remote_sql':remote}
                try:
                    if route=='direct':
                        stdout,peak=bounded_process([args.direct_worker],
                            data=json.dumps({'sql':remote,'policy':policy,'batch_bytes':budget})+'\n',
                            timeout=315,sample=True,cancel=cancel)
                        record.update(json.loads(stdout));record['process_peak']=peak
                    else:
                        result=pg('SELECT pg_backend_pid(); EXPLAIN(ANALYZE,VERBOSE,'+('TIMING OFF,' if args.timing_off else '')+'FORMAT JSON) '+sql)
                        pid,data=result.split('\n',1);plan=json.loads(data)[0];scan=foreign(plan['Plan'])
                        record.update({'backend_pid':int(pid),'wall_ms':plan['Execution Time'],'planning_ms':plan['Planning Time'],'scan':scan,'query_id':scan['FDW last query ID'],'plan':plan})
                        record['process_peak']=pg_peaks.get(int(pid),{})
                        assert scan['FDW metrics complete']
                        assert scan.get('Rows Removed by Filter',0)==0
                        assert scan['FDW peak individual batch bytes']<=budget
                    record['process_wall_ms']=(time.monotonic()-start)*1000
                except Exception as error:record['error']=redact(str(error))
                return record
              with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
                try:cohort=list(pool.map(run,range(concurrency)))
                except BaseException:
                    cancel.set()
                    raise
              def flush_logs():
                if sampler.poll() is not None:
                    raise RuntimeError('process sampler exited before profiling completed')
                ch('SYSTEM FLUSH LOGS')
              expected=125 if workload.startswith('selective_') else (32768 if workload=='wide1k' else 1000000)
              persist_cohort(out,cohort,flush_logs,querylog,expected)
              records.extend(cohort)
              print(json.dumps({'workload':workload,'policy':policy,'budget':budget,'concurrency':concurrency,'route':route,'errors':sum('error' in r for r in cohort)}),flush=True)
    sampler_reader.join(timeout=1)
    sampler.stdout.close()
    if any('error' in r for r in records):raise SystemExit(1)

if __name__=='__main__':
    try:
        main()
    except Exception as error:
        print(json.dumps({'kind':'failure','error':redact(str(error))}),flush=True)
        raise SystemExit(1) from None
