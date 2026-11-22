#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Sample actual CPU stacks of five million-row FDW scans in an owned PID namespace."""
import argparse
import json
from pathlib import Path
import shutil
import signal
import subprocess
import time
import uuid

# Each remote process has a private PID/start-time record. The start-time check
# prevents cleanup from signalling a reused PID after normal process exit.
START = '''pidfile=$1; shift; umask 077
printf '%s ' "$$" > "$pidfile"
awk '{print $22}' /proc/$$/stat >> "$pidfile"
exec "$@"'''
STOP = '''if [ -r "$1" ]; then
read -r target born < "$1" || exit 0
case "$target" in ''|*[!0-9]*) exit 1;; esac
case "$born" in ''|*[!0-9]*) exit 1;; esac
now=$(awk '{print $22}' "/proc/$target/stat" 2>/dev/null) || exit 0
if [ "$now" = "$born" ]; then kill "-$2" "$target" 2>/dev/null || :; fi
fi'''


def start_owned(container, command, **kwargs):
    marker='/tmp/fdw-perf-'+uuid.uuid4().hex+'.pid'
    process=subprocess.Popen(['docker','exec','-i',container,'sh','-c',START,'fdw-perf',marker,
                              'timeout','--signal=INT','--kill-after=5s','180s',*command],**kwargs)
    return process,container,marker


def stop_owned(owned):
    process,container,marker=owned
    failures=[]
    for signum in ['INT','TERM']:
        try:
            subprocess.run(['docker','exec',container,'sh','-c',STOP,'fdw-perf',marker,signum],
                           check=True,capture_output=True,timeout=5)
        except (OSError,subprocess.SubprocessError):
            failures.append('remote '+signum+' cleanup failed')
        try:
            process.wait(timeout=6)
            break
        except subprocess.TimeoutExpired:
            pass
    if process.poll() is None:
        process.kill();process.wait(timeout=5)
        failures.append('docker wrapper required SIGKILL; remote timeout remains bounded')
    try:
        subprocess.run(['docker','exec',container,'rm','-f',marker],check=True,capture_output=True,timeout=5)
    except (OSError,subprocess.SubprocessError):
        failures.append('remote PID marker cleanup failed')
    for stream in (process.stdin,process.stdout,process.stderr):
        if stream is not None:stream.close()
    return failures


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--pg-container',required=True);parser.add_argument('--perf-container',required=True)
    parser.add_argument('--output',required=True)
    args=parser.parse_args()
    local=Path(args.output);local.mkdir(parents=True,exist_ok=True)
    out=Path('/workspace')/local.resolve().relative_to(Path.cwd())
    owned=[];cleanup=[]
    previous=signal.getsignal(signal.SIGTERM)
    def interrupted(signum,_frame):raise SystemExit(128+signum)
    signal.signal(signal.SIGTERM,interrupted)
    try:
        pg_owned=start_owned(args.pg_container,['psql','-U','postgres','-XAtq','-v','ON_ERROR_STOP=1'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        owned.append(pg_owned);pg=pg_owned[0]
        pg.stdin.write('SELECT pg_backend_pid();\n');pg.stdin.flush();pid=int(pg.stdout.readline())
        with (local/'perf-record.log').open('w') as log:
            perf_owned=start_owned(args.perf_container,['perf','record','-e','cycles:u','-F','199','--call-graph','dwarf,16384','-p',str(pid),'-o',str(out/'million.perf.data')],stdout=log,stderr=subprocess.STDOUT)
            owned.append(perf_owned);perf=perf_owned[0]
            time.sleep(.5)
            sql="ALTER SERVER fdw_profile OPTIONS (SET terminal_policy 'in_band',SET batch_bytes '16777216');\n"+"EXPLAIN(ANALYZE,TIMING OFF,FORMAT JSON) SELECT id,url,extra FROM fdw_profile_events WHERE wc_match(url,'*normal*','raw') OR wc_match(url,'*powershell*','raw');\n"*5+'\\q\n'
            stdout,stderr=pg.communicate(sql,timeout=180)
            if pg.returncode:raise RuntimeError(stderr)
            plans=[];remaining=stdout.strip();decoder=json.JSONDecoder()
            while remaining:
                plan,end=decoder.raw_decode(remaining);plans.append(plan[0]);remaining=remaining[end:].lstrip()
            (local/'perf-explain.json').write_text(json.dumps(plans,indent=2)+'\n')
            perf.wait(timeout=30)
            if perf.returncode:raise RuntimeError('perf record failed; inspect perf-record.log')
        with (local/'perf.script').open('w') as output:
            subprocess.run(['docker','exec',args.perf_container,'timeout','--signal=TERM','--kill-after=5s','55s','perf','script','-i',str(out/'million.perf.data')],stdout=output,check=True,timeout=70)
        with (local/'perf-report.txt').open('w') as output:
            subprocess.run(['docker','exec',args.perf_container,'timeout','--signal=TERM','--kill-after=5s','55s','perf','report','--stdio','-i',str(out/'million.perf.data'),'--percent-limit','1'],stdout=output,check=True,timeout=70)
        script=local/'perf.script'
        if shutil.which('c++filt'):
            for source,target in [('perf.script','perf-demangled.script'),('perf-report.txt','perf-report-demangled.txt')]:
                with (local/source).open() as inp,(local/target).open('w') as output:
                    subprocess.run(['c++filt','-s','rust'],stdin=inp,stdout=output,check=True,timeout=30)
            script=local/'perf-demangled.script'
        subprocess.run(['python3','tests/benchmarks/perf_flamegraph.py',str(script),str(local/'flamegraph.svg')],check=True,timeout=30)
    finally:
        for child in reversed(owned):cleanup.extend(stop_owned(child))
        signal.signal(signal.SIGTERM,previous)
        (local/'cleanup.json').write_text(json.dumps({'failures':cleanup},indent=2)+'\n')
    if cleanup:raise RuntimeError('profiler child cleanup was incomplete; inspect cleanup.json')

if __name__=='__main__':main()
