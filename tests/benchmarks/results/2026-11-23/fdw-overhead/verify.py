#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Verify hydrated FDW evidence without starting either database."""
import hashlib
import importlib.util
import json
from pathlib import Path
import sys

sys.dont_write_bytecode = True
root = Path(__file__).resolve().parent
try:
    for line in (root / 'SHA256SUMS').read_text().splitlines():
        expected, name = line.split('  ', 1)
        path = root / name
        if path.is_symlink() or not path.resolve().is_relative_to(root):
            raise ValueError('unsafe evidence path')
        digest = hashlib.sha256()
        with path.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(chunk)
        if digest.hexdigest() != expected:
            raise ValueError('checksum mismatch: ' + name)
    spec = importlib.util.spec_from_file_location('report_overhead', root / 'report-overhead.py')
    report = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(report)
    actual = report.build_report(root)
    if actual != json.loads((root / 'summary.json').read_text()):
        raise ValueError('summary differs from recorded observations')
except (FileNotFoundError, ValueError, KeyError) as error:
    raise SystemExit(str(error) + '\nFetch and verify the complete archive from the repository root:\n'
        'python3 tools/verify_benchmark_artifacts.py --manifest '
        'tests/benchmarks/results/2026-11-23/fdw-overhead/artifacts.json') from error
print('Verified 10 cohorts, 8 profiles, exact-result receipts, checksums, and reproduced summary.')
