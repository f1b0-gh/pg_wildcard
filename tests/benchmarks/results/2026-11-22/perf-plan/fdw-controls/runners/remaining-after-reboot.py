"""Resume unfinished plan controls after a confirmed host reboot, never replay successes."""
import json, os, signal, subprocess, time
from pathlib import Path

root = Path('.local/perf')
status_path = root / 'execution/remaining-status.json'
status = json.loads(status_path.read_text())
assert [(x['step'], x['exit_status']) for x in status] == [('byte-index-write-cost', 0), ('fdw-c8', 0)]
receipt = json.loads((root / 'fdw/concurrency-32-interrupted-reboot/interruption.json').read_text())
assert receipt['cleanup_completed']
installed = subprocess.check_output(['docker', 'exec', 'wildcard-review-pg18', 'sha256sum', '/usr/lib/postgresql/18/lib/pg_wildcard.so'], text=True).split()[0]
assert installed == '6c23575ef8e61df9fef3e257783714f706afb904e0f7daab77544f68c08ba036'
boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
steps = [('fdw-c32', ['python3', '.local/perf/execution/fdw-benchmark.py', '--label', 'baseline-c32', '--out', '.local/perf/fdw/concurrency-32', '--workloads', 'narrow,wide1k', '--concurrency', '32', '--repeats', '1', '--policy', 'in_band'], 1800)]
for policy in ('flush_first', 'observe_first', 'observe_only', 'in_band'):
    steps.append(('fdw-' + policy, ['python3', '.local/perf/execution/fdw-benchmark.py', '--label', 'baseline-' + policy, '--out', '.local/perf/fdw/policy-' + policy, '--workloads', 'narrow', '--policy', policy, '--repeats', '3'], 900))
for budget in (1048576, 4194304):
    steps.append((f'fdw-budget-{budget}', ['python3', '.local/perf/execution/fdw-benchmark.py', '--label', f'baseline-budget-{budget}', '--out', f'.local/perf/fdw/budget-{budget}', '--workloads', 'wide1k', '--batch-bytes', str(budget), '--repeats', '3', '--policy', 'in_band'], 900))
steps.append(('remaining-matrix', ['python3', '.local/perf/execution/matrix.py'], 18000))

def save():
    tmp = status_path.with_suffix('.tmp')
    with tmp.open('w') as f:
        f.write(json.dumps(status, indent=2) + '\n'); f.flush(); os.fsync(f.fileno())
    tmp.replace(status_path)

for name, command, limit in steps:
    if (root / 'execution/timing-active').exists():
        raise RuntimeError('unexpected timer owner')
    log_path = root / 'execution' / (name + '-after-reboot.log')
    if log_path.exists():
        raise RuntimeError('refusing to overwrite attempt')
    print('START', name, flush=True)
    started = time.monotonic()
    with log_path.open('w') as log:
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            result = process.wait(timeout=limit)
        except BaseException:
            os.killpg(process.pid, signal.SIGTERM)
            try: process.wait(timeout=30)
            except subprocess.TimeoutExpired: os.killpg(process.pid, signal.SIGKILL); process.wait()
            status.append(dict(step=name, exit_status=process.returncode, classification='dispatcher interrupted or budget exceeded; inspect cleanup', boot_id=boot))
            save()
            raise
    status.append(dict(step=name, exit_status=result, seconds=time.monotonic() - started, boot_id=boot))
    save()
    print('END', name, result, flush=True)
    if result:
        if name.startswith('fdw-budget-'):
            directory = root / 'fdw' / ('budget-' + name.removeprefix('fdw-budget-'))
            clean = json.loads((directory / 'cleanup.json').read_text())['failures'] == []
            log = (directory / 'direct-0.private.log').read_text()
            if clean and 'Context { phase: Data, cause: LimitExceeded }' in log:
                status[-1]['classification'] = 'expected_data_resource_refusal; no completed latency sample'
                save()
                continue
        raise SystemExit('stop to inspect failure')
print('REMAINING CONTROLS COMPLETE', flush=True)
