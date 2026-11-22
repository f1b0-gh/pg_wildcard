"""Validate final staged bundles and regenerate reports without private state."""
import gzip
import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path

repo = Path.cwd()
root = repo / '.local/perf'
bundle_root = Path(sys.argv[1]) if len(sys.argv) > 1 else root / 'export'
assert not (root / 'execution/timing-active').exists(), 'measurement window active'
controller_path = root / 'execution/resumed-controller.json'
if controller_path.exists():
    controller = json.loads(controller_path.read_text())
    proc = Path(f"/proc/{controller['pid']}/stat")
    if proc.exists():
        stat = proc.read_text()
        assert stat[stat.rfind(')')+2:].split()[0] in ('Z', 'X'), 'controller still live'
(root / 'execution').mkdir(parents=True, exist_ok=True)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def bundle(name):
    directory = bundle_root / name
    manifest = json.loads((directory / 'sha256.json').read_text())
    actual = {str(p.relative_to(directory)) for p in directory.rglob('*') if p.is_file()}
    assert actual == set(manifest) | {'sha256.json'}, name
    payloads = {}
    for name, digest in manifest.items():
        relative = Path(name)
        assert not relative.is_absolute() and '..' not in relative.parts
        raw = (directory / relative).read_bytes()
        assert sha(raw) == digest, name
        decoded_name = name.removesuffix('.gz')
        assert decoded_name not in payloads
        payloads[decoded_name] = gzip.decompress(raw) if name.endswith('.gz') else raw
    return payloads, sha((directory / 'sha256.json').read_bytes())


def write(base, name, data):
    target = base / name
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(data)


matrix, matrix_hash = bundle('route-matrix')
fdw, fdw_hash = bundle('fdw-controls')
receipt = dict(bundle_manifest_sha256={'route-matrix': matrix_hash, 'fdw-controls': fdw_hash},
               scope='offline report regeneration from sanitized exported payloads plus public repository helpers',
               engine_access='socket connects and subprocess creation blocked inside each reporter',
               outputs={})
with tempfile.TemporaryDirectory(prefix='offline-reports-', dir=root / 'execution') as work:
    base = Path(work)
    for helper in (repo / 'tests/benchmarks').glob('*.py'):
        write(base, 'tests/benchmarks/' + helper.name, helper.read_bytes())
    write(base, 'libwildcard/tests/benchmarks/sweeps.py',
          (repo / 'libwildcard/tests/benchmarks/sweeps.py').read_bytes())
    status = json.loads(matrix['matrix-status.json'])
    completed = {r['case'] for r in status if r['status'] == 'complete'}
    for name, data in matrix.items():
        if name.split('/')[0] in completed:
            write(base, '.local/perf/matrix/' + name, data)
        elif name.startswith('runners/report-'):
            write(base, '.local/perf/execution/' + Path(name).name, data)
    write(base, '.local/perf/execution/matrix-status.json', matrix['matrix-status.json'])
    for name, data in fdw.items():
        prefix = name.split('/')[0]
        if prefix == 'runners':
            if Path(name).name.startswith('report-'):
                write(base, '.local/perf/execution/' + Path(name).name, data)
        elif name == 'control-status.json':
            write(base, '.local/perf/execution/remaining-status.json', data)
        elif name == 'pg-budget-status.json':
            write(base, '.local/perf/execution/pg-budget-status.json', data)
        elif prefix == 'interrupted-c32':
            write(base, '.local/perf/fdw/concurrency-32-interrupted-reboot/' + name.split('/', 1)[1], data)
        else:
            write(base, '.local/perf/fdw/' + name, data)
    # Do not seed the generated reports: missing source data must fail visibly.
    reports = {
        'matrix-report.md': matrix, 'matrix-report.json': matrix, 'matrix-storage.json': matrix,
        'index-tradeoffs.md': matrix, 'index-tradeoffs.json': matrix,
        'domain-terminal-observation.json': matrix,
        'fdw/concurrency-report.md': fdw, 'fdw/concurrency-report.json': fdw,
        'fdw/controls-report.md': fdw, 'fdw/controls-report.json': fdw,
    }
    for name in reports:
        (base / '.local/perf' / name).unlink(missing_ok=True)
    guard = """import runpy,sys
def guard(event, args):
    if event in ('socket.connect', 'socket.connect_ex', 'subprocess.Popen', 'os.system'):
        raise RuntimeError('offline reporter attempted external access: ' + event)
sys.addaudithook(guard)
runpy.run_path(sys.argv[1], run_name='__main__')
"""
    for script in ('report-matrix.py', 'report-index-tradeoffs.py', 'report-domain-terminal.py',
                   'report-fdw-concurrency.py', 'report-fdw-controls.py'):
        result = subprocess.run([sys.executable, '-c', guard, '.local/perf/execution/' + script],
                                cwd=base, capture_output=True, text=True, timeout=120)
        if result.returncode:
            raise RuntimeError(script + '\n' + result.stdout + '\n' + result.stderr)
    for name, payloads in reports.items():
        actual = (base / '.local/perf' / name).read_bytes()
        expected = payloads[name.removeprefix('fdw/')]
        assert actual == expected, 'report differs: ' + name
        receipt['outputs'][name] = sha(actual)
target = root / 'execution/final-bundles-offline-reproduction.json'
target.write_text(json.dumps(receipt, indent=2) + '\n')
print('Reproduced', len(receipt['outputs']), 'reports byte-for-byte without fixture state or engine access')
