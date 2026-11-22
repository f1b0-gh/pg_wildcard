#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Measure complete PostgreSQL binary COPY delivery with a bounded chunk sink.

The default host sink includes Docker data forwarding. --sink-location container
keeps COPY consumption beside psql and returns only JSON across Docker; its inner
psql process clock and outer Docker/Python process clock are reported separately.
"""
import argparse
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import sys
import threading
import time
import urllib.request
import uuid

HEADER=b'PGCOPY\n\xff\r\n\0'+b'\0'*8
TRAILER=b'\xff\xff'
CHUNK=1024*1024


def expected_bytes(workload):
    if workload not in ('narrow','broad_recheck'):raise ValueError('unknown wire workload')
    rows=1000000
    if workload=='narrow':return len(HEADER)+rows*(2+4+8)+len(TRAILER)
    normal=len('benign.test/normal'.encode());evil=len('evil.test/powershell'.encode())
    return len(HEADER)+rows*(2+(4+8)+(4+normal)+(4+16))+100*(evil-normal)+len(TRAILER)


def copy_query(workload,table):
    if not re.fullmatch(r'fdw_wire_[a-f0-9]{32}',table):raise ValueError('invalid private fixture name')
    expected_bytes(workload)
    if workload=='narrow':return f'SELECT id FROM {table}'
    return f"SELECT id,url,extra FROM {table} WHERE wc_match(url,'*normal*','raw') OR wc_match(url,'*powershell*','raw')"


def psql_command(timeout):
    return ['timeout','--signal=TERM','--kill-after=5s',str(timeout)+'s','psql','-h','127.0.0.1','-U','postgres','-d','postgres','-XAtq','-v','ON_ERROR_STOP=1']


def consume_copy(command,query,workload,timeout):
    """No per-row parsing or retained result; only header, tail and byte count."""
    result={};total=0;prefix=b'';tail=b'';first=None
    start=time.monotonic()
    try:
        with subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE) as process:
            expired=threading.Event()
            def stop_stalled_wrapper():
                if process.poll() is None:
                    expired.set()
                    try:process.kill()
                    except ProcessLookupError:pass
            watchdog=threading.Timer(timeout+10,stop_stalled_wrapper)
            watchdog.daemon=True;watchdog.start()
            try:
                process.stdin.write(('COPY ('+query+') TO STDOUT (FORMAT binary);\n').encode());process.stdin.close()
                prefix=process.stdout.read(len(HEADER));first=time.monotonic()
                total=len(prefix);tail=prefix[-2:]
                while chunk:=process.stdout.read(CHUNK):
                    total+=len(chunk);tail=(tail+chunk)[-2:] if len(chunk)<2 else chunk[-2:]
                stderr=process.stderr.read().decode(errors='replace')
                status=process.wait(timeout=5)
            except BaseException:
                if process.poll() is None:process.kill()
                process.wait(timeout=5)
                raise
            finally:
                watchdog.cancel();watchdog.join(timeout=1)
        result.update({'process_wall_ms':(time.monotonic()-start)*1000,'first_copy_header_ms':(first-start)*1000 if prefix==HEADER else None,'copy_bytes':total,'header_valid':prefix==HEADER,'trailer_valid':tail==TRAILER,'exit_status':status})
        if expired.is_set():raise RuntimeError('COPY transfer deadline exceeded; remote psql retains its own timeout')
        if status:raise RuntimeError(stderr)
        if prefix!=HEADER or tail!=TRAILER or total!=expected_bytes(workload):raise RuntimeError('COPY binary oracle mismatch')
        result['rows_per_second']=1000000000/result['process_wall_ms']
    except Exception as error:
        result['error']=str(error)
        result.setdefault('process_wall_ms',(time.monotonic()-start)*1000)
        result.setdefault('copy_bytes',total)
    return result


def copy_worker():
    # Invoked inside the PG container. Credentials never enter this command.
    raw=sys.stdin.buffer.read(65537)
    if len(raw)>65536:raise ValueError('worker command exceeds limit')
    request=json.loads(raw)
    timeout=request.get('timeout_seconds')
    if type(timeout) is not int or not 1<=timeout<=300:raise ValueError('invalid worker timeout')
    workload=request['workload'];query=copy_query(workload,request['table'])
    print(json.dumps(consume_copy(psql_command(timeout),query,workload,timeout)),flush=True)


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--pg-container',required=True);parser.add_argument('--output',required=True)
    parser.add_argument('--repeats',type=int,default=3);parser.add_argument('--timeout-seconds',type=int,default=60)
    parser.add_argument('--sink-location',choices=['host','container'],default='host')
    args=parser.parse_args()
    if not 1<=args.repeats<=20 or not 1<=args.timeout_seconds<=300:raise ValueError('invalid run bound')
    out=Path(args.output);out.mkdir(parents=True,exist_ok=True)
    if (out/'raw-copy.jsonl').exists():raise ValueError('choose a fresh output directory')
    token=os.environ['ANTALYA_TEST_TOKEN'];url=os.environ['ANTALYA_TEST_URL']
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    def redact(value):
        value=str(value)
        for key in ['ANTALYA_TEST_TOKEN','ANTALYA_TEST_URL','FDW_ENGINE_URL']:
            private=os.environ.get(key)
            if private:value=value.replace(private,'[redacted]')
        return value
    def ch(sql):
        request=urllib.request.Request(url,data=sql.encode(),headers={'Authorization':'Bearer '+token})
        with opener.open(request,timeout=args.timeout_seconds) as response:return response.read().decode()
    psql=['docker','exec','-i',args.pg_container,*psql_command(args.timeout_seconds)]
    def pg(sql):
        result=subprocess.run(psql,input=sql.encode(),capture_output=True,timeout=args.timeout_seconds+10)
        if result.returncode:raise RuntimeError(redact(result.stderr.decode(errors='replace')))
        return result.stdout.decode().strip()
    boundary=('Inner psql spawn through completion, TCP loopback PG protocol and container-local chunk consumption; Docker/Python process elapsed is separately recorded.' if args.sink_location=='container' else 'Before docker exec/psql start through process completion, including TCP loopback PG protocol and Docker stdout forwarding/host consumption.')
    settings={'postgres':pg('SELECT version()'),'engine':ch('SELECT version()').strip(),'repeats':args.repeats,'sink_location':args.sink_location,'remote_timeout_seconds':args.timeout_seconds,'copy_deadline_seconds':args.timeout_seconds+10,'sink_chunk_bytes':CHUNK,'rows_per_operation':1000000,'terminal_policy':'flush_first','batch_bytes':16777216,'clock_boundary':boundary+' Not identical to EXPLAIN executor or direct-client fetch clocks.','first_header':'Offset until the complete 19-byte COPY header is received; not HTTP TTFB.','byte_boundary':'COPY payload only; PostgreSQL message/TCP framing bytes are not counted.','oracle':'Exact deterministic COPY bytes/header/trailer, successful psql exit, unique native QueryFinish result_rows=1000000. No per-row Python parsing.'}
    (out/'settings.json').write_text(json.dumps(settings,indent=2)+'\n')
    pg("ALTER SERVER fdw_profile OPTIONS (SET terminal_policy 'flush_first', SET batch_bytes '16777216')")
    failures=0;records=[]
    for workload in ['narrow','broad_recheck']:
      for repeat in range(args.repeats):
        identifier='fdw_wire_'+uuid.uuid4().hex;marker=identifier.replace('_','-')
        pg(f"CREATE FOREIGN TABLE {identifier}(id bigint,url text,extra text) SERVER fdw_profile OPTIONS(table 'fdw_profile.events /* {marker} */')")
        route='postgres_binary_copy_tcp_loopback_'+('container_sink' if args.sink_location=='container' else 'via_docker')
        record={'route':route,'workload':workload,'repeat':repeat,'expected_rows':1000000,'expected_copy_bytes':expected_bytes(workload),'native_marker':marker}
        try:
            if args.sink_location=='container':
                command=['docker','exec','-i',args.pg_container,'timeout','--signal=TERM','--kill-after=5s',str(args.timeout_seconds+15)+'s','python3','/workspace/tests/benchmarks/fdw_wire.py','--copy-worker']
                outer=time.monotonic()
                worker=subprocess.run(command,input=json.dumps({'table':identifier,'workload':workload,'timeout_seconds':args.timeout_seconds}).encode(),capture_output=True,timeout=args.timeout_seconds+25)
                record['outer_docker_process_wall_ms']=(time.monotonic()-outer)*1000
                if worker.returncode:raise RuntimeError(redact(worker.stderr.decode(errors='replace')))
                record.update(json.loads(redact(worker.stdout.decode())))
            else:
                result=consume_copy(psql,copy_query(workload,identifier),workload,args.timeout_seconds)
                if 'error' in result:result['error']=redact(result['error'])
                record.update(result)
        except Exception as error:record['error']=redact(error)
        finally:
            pending=out/(identifier+'.pending.json');pending.write_text(json.dumps(record,indent=2)+'\n')
        try:
            ch('SYSTEM FLUSH LOGS')
            native=json.loads(ch("SELECT query_id,query_duration_ms,read_rows,read_bytes,result_rows,result_bytes,memory_usage,ProfileEvents FROM system.query_log WHERE type='QueryFinish' AND startsWith(query_id,'wildcard-') AND position(query,'"+marker+"')>0 FORMAT JSON"))['data']
            record['query_log']=native
            if len(native)!=1 or int(native[0]['result_rows'])!=1000000:raise RuntimeError('native COPY correlation mismatch')
            record['query_id']=native[0]['query_id'];record['server_ms']=int(native[0]['query_duration_ms'])
        except Exception as error:record['control_error']=redact(error)
        finally:
            try:pg(f'DROP FOREIGN TABLE {identifier}')
            except Exception as error:record['cleanup_error']=redact(error)
        with (out/'raw-copy.jsonl').open('a') as f:f.write(json.dumps(record)+'\n')
        pending.unlink();records.append(record)
        failed=any(key in record for key in ['error','control_error','cleanup_error']);failures+=failed
        print(json.dumps({'workload':workload,'repeat':repeat,'sink':args.sink_location,'ok':not failed,'copy_bytes':record.get('copy_bytes'),'process_wall_ms':record.get('process_wall_ms')}),flush=True)
        if 'cleanup_error' in record:raise RuntimeError('wire fixture cleanup failed; measured/control records retained in raw-copy.jsonl')
    summary=[]
    for workload in ['narrow','broad_recheck']:
        successful=[r for r in records if r['workload']==workload and not any(k in r for k in ['error','control_error','cleanup_error'])]
        if successful:
            wall=statistics.median(r['process_wall_ms'] for r in successful)
            row={'workload':workload,'samples':len(successful),'rows':1000000,'copy_bytes':expected_bytes(workload),'process_wall_ms':wall,'rows_per_second':1000000000/wall,'first_copy_header_ms':statistics.median(r['first_copy_header_ms'] for r in successful),'server_ms':statistics.median(r['server_ms'] for r in successful)}
            if args.sink_location=='container':row['outer_docker_process_wall_ms']=statistics.median(r['outer_docker_process_wall_ms'] for r in successful)
            summary.append(row)
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    if failures:raise SystemExit(1)

if __name__=='__main__':
    try:
        if sys.argv[1:]==['--copy-worker']:copy_worker()
        else:main()
    except Exception as error:
        message=str(error)
        for key in ['ANTALYA_TEST_TOKEN','ANTALYA_TEST_URL','FDW_ENGINE_URL']:
            value=os.environ.get(key)
            if value:message=message.replace(value,'[redacted]')
        raise SystemExit(message) from None
