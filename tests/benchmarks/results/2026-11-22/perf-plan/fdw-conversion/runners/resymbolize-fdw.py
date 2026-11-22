"""Resolve saved FDW CPU samples against exact binaries, without replaying queries."""
import hashlib
import json
import shutil
import subprocess
import xml.etree.ElementTree as ET
from pathlib import Path

root = Path('.local/perf/fdw')
binaries = {'baseline': Path('.local/perf-review/fdw/pg_wildcard-measured-release.so'),
            'prepared': Path('.local/perf/execution/pg-prepared.so')}
status = json.loads((root / 'flames/status.json').read_text())
if len(status) != 2 or {r['label'] for r in status} != set(binaries) or any(r['exit_status'] for r in status):
    raise ValueError('both completed recordings required')
records = []

def folded_count(path):
    return sum(int(line.rsplit(' ', 1)[1]) for line in path.read_text().splitlines())

for entry in status:
    label = entry['label']
    binary = binaries[label]
    binary_hash = hashlib.sha256(binary.read_bytes()).hexdigest()
    if binary_hash != entry['binary_sha256']:
        raise ValueError('saved binary hash differs from recorded profile')
    symroot = root / 'symbol-roots' / label
    mappings = {'usr/lib/postgresql/18/lib/pg_wildcard.so': '/workspace/' + str(binary)}
    for path in ('usr/lib/postgresql/18/bin/postgres', 'usr/lib/x86_64-linux-gnu/libc.so.6',
                 'usr/lib/x86_64-linux-gnu/libm.so.6', 'usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2'):
        mappings[path] = '/proc/1/root/' + path
    for path, target in mappings.items():
        link = symroot / path
        link.parent.mkdir(parents=True, exist_ok=True)
        if not link.is_symlink():
            link.symlink_to(target)
        elif str(link.readlink()) != target:
            raise ValueError('unexpected symbol mapping')
    for workload in ('narrow', 'smalltext', 'wide1k', 'broad_recheck'):
        directory = root / 'flames' / label / workload
        previous = directory / 'original-unresolved'
        previous.mkdir(exist_ok=False)
        old_hashes = {}
        for name in ('perf.script', 'perf-demangled.script', 'perf-report.txt', 'flamegraph.svg', 'flamegraph.folded'):
            old_hashes[name] = hashlib.sha256((directory / name).read_bytes()).hexdigest()
            shutil.move(directory / name, previous / name)
        remote = '/workspace/' + str(directory / 'cpu.perf.data')
        with (directory / 'resymbolize.log').open('w') as log:
            for command, output_name in (('script', 'perf.script'), ('report', 'perf-report.txt')):
                command_args = ['docker', 'exec', 'wildcard-perf-plan-helper', 'perf', command,
                    '-i', remote, '--symfs', '/workspace/' + str(symroot)]
                if command == 'report':
                    command_args += ['--stdio', '--percent-limit', '1']
                with (directory / output_name).open('w') as output:
                    subprocess.run(command_args, stdout=output, stderr=log, check=True, timeout=60)
            with (directory / 'perf.script').open() as inp, (directory / 'perf-demangled.script').open('w') as out:
                subprocess.run(['c++filt', '-s', 'rust'], stdin=inp, stdout=out, stderr=log, check=True, timeout=30)
            subprocess.run(['python3', 'tests/benchmarks/perf_flamegraph.py',
                str(directory / 'perf-demangled.script'), str(directory / 'flamegraph.svg')],
                stdout=log, stderr=log, check=True, timeout=30)
        ET.parse(directory / 'flamegraph.svg')
        old_samples = folded_count(previous / 'flamegraph.folded')
        new_samples = folded_count(directory / 'flamegraph.folded')
        if old_samples != new_samples:
            raise ValueError('symbolization changed sample count')
        frames = [line for line in (directory / 'perf.script').read_text().splitlines()
                  if '/pg_wildcard.so)' in line]
        unresolved = sum('[unknown]' in line for line in frames)
        if not frames or unresolved / len(frames) > 0.05:
            raise ValueError('extension symbol resolution remains inadequate')
        records.append(dict(label=label, workload=workload, samples=new_samples,
            extension_frames=len(frames), unresolved_extension_frames=unresolved,
            binary_sha256=binary_hash, original_derived_sha256=old_hashes,
            symbol_root='/workspace/' + str(symroot), symbol_mappings=mappings))
        print(label, workload, new_samples, 'samples;', unresolved, '/', len(frames), 'extension frames unresolved', flush=True)
(root / 'flames/symbolization.json').write_text(json.dumps(dict(
    method='Same saved perf recordings; stable symbol root with exact per-label extension binary. No query replay or new CPU samples.',
    observation='Original --symfs /proc/1/root left extension frames unresolved. That path canonicalizes to / in the helper. Stable per-binary roots resolve the recorded frames.',
    old_outputs='Preserved privately under each original-unresolved directory.',
    postprocessor_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    profiles=records), indent=2) + '\n')
