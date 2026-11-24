#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Matched native/antalya-rs sweeps; owned fixture only, exact checks after timing."""
import argparse
import concurrent.futures
import copy
import hashlib
import json
import os
import select
from pathlib import Path
import statistics
import sys
import threading
import time
import uuid

from comparison import corpus_digest, expected_pairs, verify_pairs
from compare_routes import emit, native_run, source_hash, storage, sweeps, cancel_native_queries
from indexed_ingest import native_setup, select_spec
from native_parallel import run as native_batches
from process_sampler import status
from route_workers import DirectWorker
import fixtures

ROUTES = ('native_single','flush_first_single','in_band_single',
          'native_parallel','flush_first_parallel','in_band_parallel',
          'native_staged','flush_first_staged','in_band_staged','in_band_observe_staged')

class Worker(DirectWorker):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        try:os.set_blocking(self.process.stdin.fileno(), False)
        except BaseException:self.close();raise

    def request_plan(self, plan):
        data=json.dumps(plan).encode()+b'\n'
        if len(data)>4<<20:raise ValueError('command limit')
        start=time.monotonic()
        deadline=start+180
        pending=memoryview(data)
        while pending:
            remaining=deadline-time.monotonic()
            if remaining<=0 or not select.select([], [self.process.stdin], [], remaining)[1]:
                raise TimeoutError('worker input deadline')
            try:written=os.write(self.process.stdin.fileno(),pending)
            except BlockingIOError:continue
            pending=pending[written:]
        result=self.read(max(.001,deadline-time.monotonic()))
        result['worker_roundtrip_ms']=(time.monotonic()-start)*1000
        return result


def plan(engine,spec,route):
    staged=route.endswith('_staged')
    single=route.endswith('_single')
    result={'database':engine.database,'queries':spec['queries'] if single else spec['parallel_dispatch']['queries'],
            'workers':1 if single else 4,'source_rows':spec['rows']}
    if staged:
        source=spec['staged_dispatch']
        result['staged']={key:source[key] for key in ('candidate_select','queries')}
    return result


def cpu(pid):
    try:
        fields=Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()
        return (int(fields[11])+int(fields[12]))/os.sysconf('SC_CLK_TCK')*1000
    except (OSError,ValueError):return None


def observe(pid,action):
    stop=threading.Event();samples=[]
    def sample():
        while not stop.is_set():
            row=status(pid)
            if row:samples.append(row)
            stop.wait(.01)
    before=cpu(pid);thread=threading.Thread(target=sample);thread.start()
    try:result=action()
    finally:stop.set();thread.join()
    after=cpu(pid)
    result['process_cpu_ms']=None if before is None or after is None else after-before
    result['sampled_rss_kib']=max((s['rss_kib'] for s in samples),default=0)
    result['sampled_threads']=max((s['threads'] for s in samples),default=0)
    result['process_hwm_kib']=max((s['hwm_kib'] for s in samples),default=0)
    return result


def observer_query(engine,sql):
    # Observers must never look like unfinished sweep work to the next check.
    return engine.query(sql,query_id='wc-observer-'+uuid.uuid4().hex,logging=False)


def server_time(engine):
    return int(observer_query(engine,'SELECT toUnixTimestamp64Micro(now64(6))')[0][0])


def profiles(engine,begin,end):
    observer_query(engine,'SYSTEM FLUSH LOGS')
    sql=f"""SELECT query_id,query_duration_ms,read_rows,read_bytes,result_rows,result_bytes,memory_usage,ProfileEvents,Settings,peak_threads_usage,
      multiIf(startsWith(query,'SYSTEM FLUSH'),'flush',startsWith(query,'SELECT'),'select','command')
      FROM system.query_log WHERE type='QueryFinish' AND is_initial_query=1
      AND query_start_time_microseconds>=fromUnixTimestamp64Micro({begin})
      AND query_start_time_microseconds<=fromUnixTimestamp64Micro({end})
      AND (startsWith(query_id,{sweeps.literal(engine.prefix)}) OR startsWith(query_id,'antalya-probe-') OR (current_database={sweeps.literal(engine.database)} AND startsWith(query_id,'comparison-direct-')))
      ORDER BY query_start_time_microseconds,query_id"""
    fields=('query_id','query_duration_ms','read_rows','read_bytes','result_rows','result_bytes','memory_usage','profile_events','settings','peak_threads_usage','kind')
    return [dict(zip(fields,row)) for row in observer_query(engine,sql)]


def assert_clean(engine):
    active=observer_query(engine,'SELECT query_id,elapsed,substring(query,1,160) FROM system.processes WHERE query_id!=currentQueryID() AND current_database='+sweeps.literal(engine.database)+" AND (startsWith(query_id,'comparison-direct-') OR startsWith(query_id,"+sweeps.literal(engine.prefix)+'))')
    tables=int(observer_query(engine,"SELECT count() FROM system.tables WHERE database="+sweeps.literal(engine.database)+" AND name='wc_staged'")[0][0])
    if active or tables:raise AssertionError(f'cleanup failed: active={active}, staging={tables}')


def cleanup(workers,engine,state,out,failed):
    errors=[]
    for worker in workers:
        try:worker.close()
        except Exception as error:
            errors.append(error)
            try:
                if worker.process.poll() is None:worker.process.kill()
                worker.process.wait(timeout=5)
            except Exception as reap_error:errors.append(reap_error)
    if engine:
        for action in (lambda:cancel_native_queries(engine),engine.close):
            try:action()
            except Exception as error:errors.append(error)
    teardown=failed or bool(errors)
    if teardown:
        try:fixtures.stop(state)  # Verifies container/network labels before removal.
        except Exception as error:errors.append(error)
    (out/'cleanup.json').write_text(json.dumps({'fixture_teardown_requested':teardown,
        'errors':[type(e).__name__ for e in errors],'completed':not errors},indent=2)+'\n')
    return errors


def qualify(engine,spec,workers,out):
    expected=expected_pairs(spec)
    for policy,worker in workers.items():
        for shape in ('single','parallel','staged'):
            r=worker.request_plan(plan(engine,spec,policy+'_'+shape))
            if 'error' in r:raise AssertionError(r)
            verify_pairs([tuple(p) for p in r['pairs']],expected);assert_clean(engine)
        # Empty population is a complete fallback-free result, without CREATE.
        engine.query('CREATE TABLE empty AS bench ENGINE=Memory')
        empty=copy.deepcopy(spec);empty['rows']=0
        empty['parallel_dispatch']['queries']=[q.replace('FROM bench\n','FROM empty\n') for q in empty['parallel_dispatch']['queries']]
        empty['staged_dispatch']['candidate_select']=empty['staged_dispatch']['candidate_select'].replace('FROM bench\n','FROM empty\n')
        r=worker.request_plan(plan(engine,empty,policy+'_staged'))
        if r.get('error') or r.get('pairs')!=[] or r['candidate_rows']!=0:raise AssertionError(r)
        if any(x['kind']=='create' for x in r['executions']):raise AssertionError('empty staging allocated')
        engine.query('DROP TABLE empty SYNC');assert_clean(engine)
        # Every rule appears in this dense canary; admission must refuse before CREATE.
        engine.query('CREATE TABLE canary AS bench ENGINE=Memory')
        engine.query('INSERT INTO canary SELECT toInt64(id),v FROM templates')
        dense=copy.deepcopy(spec);dense['rows']=len(spec['templates'])
        dense['parallel_dispatch']['queries']=[q.replace('FROM bench\n','FROM canary\n') for q in dense['parallel_dispatch']['queries']]
        dense['staged_dispatch']['candidate_select']=dense['staged_dispatch']['candidate_select'].replace('FROM bench\n','FROM canary\n')
        r=worker.request_plan(plan(engine,dense,policy+'_staged'))
        if r.get('error') or not r['staging_refused'] or any(x['kind']=='create' for x in r['executions']):raise AssertionError(r)
        canary=[(row,int(hit[0]),str(hit[1])) for row,hits in enumerate(spec['template_hits']) for hit in hits]
        verify_pairs([tuple(p) for p in r['pairs']],canary)
        if len({p[1] for p in canary})!=spec['indicators']:raise AssertionError('canary does not cover all rules')
        engine.query('DROP TABLE canary SYNC');assert_clean(engine)
        # Hold real server queries open, observing two waves of four. The
        # assertion uses observed overlap, not the requested worker count.
        overlap_plan=plan(engine,spec,policy+'_parallel')
        overlap_plan['queries']=["SELECT toInt64(1),toInt64(1),'raw' WHERE sleep(2)=0"]*8
        peak=0
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
            future=pool.submit(worker.request_plan,overlap_plan)
            while not future.done():
                active=int(observer_query(engine,"SELECT count() FROM system.processes WHERE current_database="+sweeps.literal(engine.database)+" AND startsWith(query_id,'comparison-direct-')")[0][0])
                peak=max(peak,active);time.sleep(.02)
            r=future.result()
        if r.get('error') or r['pairs']!=[[1,1,'raw']]*8 or peak!=4:raise AssertionError({'peak':peak,'result':r})
        assert_clean(engine)
        # A peer fails while admitted peers are blocked in engine execution.
        invalid=plan(engine,spec,policy+'_staged')
        invalid['staged']['queries']=["SELECT toInt64(1),toInt64(1),'raw' FROM wc_staged WHERE sleep(2)=0 LIMIT 1"]*3+['SELECT missing_column FROM wc_staged']
        r=worker.request_plan(invalid)
        if 'error' not in r or not r['cleanup_confirmed']:raise AssertionError(r)
        assert_clean(engine)
        # Operation timeout must cancel and join, then allow a fresh command.
        timed=plan(engine,spec,policy+'_parallel');timed['deadline_ms']=100
        timed['queries']=["SELECT toInt64(1),toInt64(1),'raw' FROM bench WHERE sleep(2)=0 LIMIT 1"]*4
        r=worker.request_plan(timed)
        if 'error' not in r or not r['cleanup_confirmed']:raise AssertionError(r)
        assert_clean(engine)
        r=worker.request_plan(plan(engine,spec,policy+'_single'))
        if 'error' in r:raise AssertionError(r)
        verify_pairs([tuple(p) for p in r['pairs']],expected)
        emit(out,kind='qualification',policy=policy,status='passed',observed_active_query_peak=peak,
             checks=['exact single/parallel/staged','empty admission','dense refusal before CREATE with all-rule canary','two waves of four queries','peer-error cancellation','deadline cancellation','worker reuse','no active query or staging table'])


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('spec',type=Path);p.add_argument('--native-state',type=Path,required=True)
    p.add_argument('--worker',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    p.add_argument('--repeats',type=int,default=7);p.add_argument('--rows',type=int)
    p.add_argument('--qualify',action='store_true');p.add_argument('--routes',default=','.join(ROUTES))
    args=p.parse_args();routes=args.routes.split(',')
    if not 1<=args.repeats<=20 or len(set(routes))!=len(routes) or set(routes)-set(ROUTES):raise ValueError('routes/repetitions')
    spec=select_spec(json.loads(args.spec.read_text()))
    if args.rows is not None:spec['rows']=args.rows
    if args.out.exists():raise ValueError('fresh output required')
    args.out.mkdir(parents=True)
    (args.out/'spec.json').write_text(json.dumps(spec))
    private=json.loads(args.native_state.read_text())
    # Reuse fixture ownership verification before sending any request.
    import subprocess
    info=json.loads(subprocess.check_output(['docker','inspect',private['name']]))[0]
    if info['Config']['Labels'].get('wildcard.benchmark.owner')!=private['owner']:raise ValueError('unowned fixture')
    engine=None;workers={};records=[];failed=True
    try:
        engine=sweeps.Engine(args.native_state,60)
        native_setup(engine,spec,args.out,emit=emit,storage=storage,source_hash=source_hash,literal=sweeps.literal,durability='defaults')
        for policy in ('flush_first','in_band','in_band_observe'):
            if any(route.startswith(policy) for route in routes) or args.qualify:
                workers[policy]=Worker(args.worker.resolve(),private,args.out/(policy+'.private.log'),extra_args=(policy,))
        metadata={'source_sha256':corpus_digest(spec),'worker_sha256':hashlib.sha256(args.worker.read_bytes()).hexdigest(),
            'runner_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'server_image':info['Image'],
            'server_cpus':info['HostConfig']['NanoCpus']/1e9,'server_memory_bytes':info['HostConfig']['Memory'],
            'rows':spec['rows'],'rules':spec['indicators'],'routes':routes,'settings':engine.settings,
            'client_setup':{key:value.ready for key,value in workers.items()},
            'clock':'native complete request; Rust internal request and full IPC roundtrip both retained; validation after timing',
            'limits':'64 MiB shared Arrow leases per client; pair count 100000 across tasks; these are not process RSS caps',
            'metrics':'component durations overlap; process CPU includes worker serialization; HWM is process-lifetime, RSS is sampled'}
        (args.out/'settings.json').write_text(json.dumps(metadata,indent=2)+'\n')
        if args.qualify:qualify(engine,spec,workers,args.out)
        expected=expected_pairs(spec)
        for round_no in range(args.repeats+1):
            order=routes[round_no%len(routes):]+routes[:round_no%len(routes)]
            for route in order:
                policy=route.rsplit('_',1)[0];shape=route.rsplit('_',1)[1]
                if policy=='native':
                    if shape=='single':action=lambda:native_run(engine,spec['queries'])
                    else:action=lambda:native_batches(engine,spec['parallel_dispatch'],source_rows=spec['rows'],staged=spec['staged_dispatch'] if shape=='staged' else None)
                    pid=os.getpid()
                else:
                    worker=workers[policy];command=plan(engine,spec,route)
                    action=lambda:worker.request_plan(command);pid=worker.process.pid
                begin=server_time(engine)
                r=observe(pid,action)
                end=server_time(engine)
                if 'error' in r:raise AssertionError(r)
                pairs=[tuple(pair) for pair in r.pop('pairs')]
                pair_hash=verify_pairs(pairs,expected)
                ids={e['query_id'] for e in r.get('executions',[])}|set(r.get('query_ids',[]))
                controls=sum(e['control_requests'] for e in r.get('executions',[]))
                # Wait outside timing for every data/command and SDK control
                # record, including a last request not queued at the first flush.
                for attempt in range(20):
                    query_profiles=profiles(engine,begin,end)
                    missing=ids-{x['query_id'] for x in query_profiles}
                    logged_controls=sum(x['query_id'].startswith('antalya-probe-') for x in query_profiles)
                    if not missing and logged_controls==controls:break
                    if logged_controls>controls:raise AssertionError('foreign control traffic in owned fixture')
                    time.sleep(.1)
                else:raise AssertionError(f'incomplete server evidence: {len(missing)} operations; {logged_controls}/{controls} controls')
                assert_clean(engine)
                record=dict(kind='sample',route=route,round=round_no,warmup=round_no==0,pairs=len(pairs),pair_sha256=pair_hash,
                            profiles=query_profiles,**r)
                records.append(record);emit(args.out,**record)
        summary=[]
        for route in routes:
            rows=[r for r in records if r['route']==route and not r['warmup']]
            item={'route':route,'samples':len(rows)}
            for key in ('request_ms','worker_roundtrip_ms','process_cpu_ms','sampled_rss_kib'):
                values=[r[key] for r in rows if r.get(key) is not None]
                if values:item[key]={'median':statistics.median(values),'min':min(values),'max':max(values)}
            summary.append(item)
        (args.out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
        failed=False
        print('COHORT PASS',flush=True)
    finally:
        errors=cleanup(workers.values(),engine,args.native_state,args.out,failed)
        for log in args.out.glob('*.private.log'):log.unlink(missing_ok=True)
        if errors:
            original=sys.exception()
            if original:original.add_note('Cleanup errors: '+', '.join(type(e).__name__ for e in errors))
            else:raise ExceptionGroup('Benchmark cleanup failed',errors)

if __name__=='__main__':main()
