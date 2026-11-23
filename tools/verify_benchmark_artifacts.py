#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Fetch pinned evidence once, then reproduce original reports without engines."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / 'tests/benchmarks/artifacts.json'
CHUNK = 1024 * 1024


def validate_manifest(manifest):
    if not isinstance(manifest, dict) or manifest.get('schema_version', 1) != 1:
        raise ValueError('unsupported archive manifest schema')
    digest = manifest.get('sha256')
    if not isinstance(digest, str) or not re.fullmatch('[0-9a-f]{64}', digest):
        raise ValueError('invalid archive checksum')
    for key, cap in [('size', 128 << 20), ('tar_size', 256 << 20),
                     ('unpacked_size', 256 << 20), ('files', 10000)]:
        value = manifest.get(key)
        if type(value) is not int or not 0 < value <= cap:
            raise ValueError('invalid archive bound: ' + key)
    url = manifest.get('url')
    if not isinstance(url, str) or not url.startswith('https://'):
        raise ValueError('archive URL must use HTTPS')
    verifiers = manifest.get('verifiers', [])
    if not isinstance(verifiers, list) or len(verifiers) > 20:
        raise ValueError('invalid verifier list')
    for script in verifiers:
        if not isinstance(script, str):
            raise ValueError('invalid verifier path')
        path = PurePosixPath(script)
        if (path.parts[:3] != ('tests', 'benchmarks', 'results') or '..' in path.parts
                or str(path) != script or '\\' in script or path.suffix != '.py'):
            raise ValueError('invalid verifier path')
    if type(manifest.get('legacy_reproduction', False)) is not bool:
        raise ValueError('invalid legacy reproduction setting')


def check_archive(path, manifest):
    if path.is_symlink() or not path.is_file() or path.stat().st_size != manifest['size']:
        raise ValueError('archive size differs; use --archive with the published asset to repair the cache')
    digest = hashlib.sha256()
    with path.open('rb') as source:
        remaining = manifest['size']
        while chunk := source.read(min(CHUNK, remaining + 1)):
            remaining -= len(chunk)
            if remaining < 0:
                raise ValueError('archive size exceeds manifest')
            digest.update(chunk)
    if remaining or digest.hexdigest() != manifest['sha256']:
        raise ValueError('archive checksum differs; use --archive with the published asset to repair the cache')


def ensure_archive(manifest, cache, source=None, offline=False):
    validate_manifest(manifest)
    cache.mkdir(parents=True, exist_ok=True)
    if cache.is_symlink():
        raise ValueError('cache must not be a symlink')
    target = cache / (manifest['sha256'] + '.tar.gz')
    if source is None and target.exists():
        check_archive(target, manifest)
        return target
    if source is None and offline:
        raise ValueError('offline cache is missing: run once online or supply --archive PATH')
    # Install only a complete, checked archive. Failed downloads leave no cache entry.
    with tempfile.NamedTemporaryFile(dir=cache, prefix='.download-', delete=False) as output:
        temporary = Path(output.name)
        try:
            if source is not None:
                stream = source.open('rb')
            else:
                request = urllib.request.Request(manifest['url'], headers={'User-Agent': 'pg-wildcard-evidence'})
                stream = urllib.request.urlopen(request, timeout=30)
                if not stream.url.startswith('https://'):
                    stream.close()
                    raise ValueError('archive redirect must use HTTPS')
            with stream:
                size = 0
                deadline = time.monotonic() + 120
                while chunk := stream.read(min(CHUNK, manifest['size'] - size + 1)):
                    size += len(chunk)
                    if size > manifest['size'] or time.monotonic() > deadline:
                        raise ValueError('archive download exceeds size or time bound')
                    output.write(chunk)
            output.flush()
            check_archive(temporary, manifest)
            os.replace(temporary, target)
        finally:
            temporary.unlink(missing_ok=True)
    return target


def extract_archive(archive, destination, manifest):
    check_archive(archive, manifest)
    if destination.is_symlink() or (destination.exists() and any(destination.iterdir())):
        raise ValueError('extraction destination must be empty and must not be a symlink')
    destination.mkdir(parents=True, exist_ok=True)
    # Bound gzip output before tarfile parses headers, including extended headers.
    with tempfile.TemporaryFile(dir=destination.parent) as decoded:
        total = 0
        with gzip.open(archive, 'rb') as compressed:
            while chunk := compressed.read(min(CHUNK, manifest['tar_size'] - total + 1)):
                total += len(chunk)
                if total > manifest['tar_size']:
                    raise ValueError('decompressed tar exceeds bound')
                decoded.write(chunk)
        if total != manifest['tar_size']:
            raise ValueError('decompressed tar size differs')
        decoded.seek(0)
        seen = set()
        unpacked = 0
        with tarfile.open(fileobj=decoded, mode='r|') as contents:
            for member in contents:
                path = PurePosixPath(member.name)
                if (not member.isfile() or member.issparse() or path.is_absolute()
                        or '..' in path.parts or str(path) != member.name
                        or '\\' in member.name
                        or path.parts[:3] != ('tests', 'benchmarks', 'results')
                        or len(path.parts) < 4):
                    raise ValueError('unsafe archive member')
                if member.name in seen:
                    raise ValueError('duplicate archive member')
                seen.add(member.name)
                unpacked += member.size
                if member.size < 0 or len(seen) > manifest['files'] or unpacked > manifest['unpacked_size']:
                    raise ValueError('archive payload exceeds bound')
                target = destination / path
                target.parent.mkdir(parents=True, exist_ok=True)
                with contents.extractfile(member) as source, target.open('xb') as output:
                    shutil.copyfileobj(source, output, CHUNK)
        if len(seen) != manifest['files'] or unpacked != manifest['unpacked_size']:
            raise ValueError('archive membership or payload size differs')


def verify_reports(extracted, manifest):
    validate_manifest(manifest)
    results = extracted / 'tests/benchmarks/results'
    environment = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    for script in manifest.get('verifiers', []):
        print('Verifying ' + script, flush=True)
        subprocess.run([sys.executable, '-B', str(extracted / script)], cwd=extracted,
                       env=environment, check=True, timeout=300)
    if not manifest.get('legacy_reproduction', False):
        return
    # The historical route/control reporter imports the public harness. Copy it
    # into the disposable checkout so its .local output never touches user work.
    for source in (ROOT / 'tests/benchmarks').glob('*.py'):
        shutil.copyfile(source, extracted / 'tests/benchmarks' / source.name)
    target = extracted / 'libwildcard/tests/benchmarks/sweeps.py'
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / 'libwildcard/tests/benchmarks/sweeps.py', target)
    plan = results / '2026-11-22/perf-plan'
    print('Reproducing route-matrix and FDW-control reports', flush=True)
    subprocess.run([sys.executable, '-B', str(plan / 'reproduction/reproduce-reports.py'), str(plan)],
                   cwd=extracted, env=environment, check=True, timeout=300)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, default=MANIFEST, help='archive manifest (default: original November evidence)')
    parser.add_argument('--offline', action='store_true', help='use only the verified local cache or --archive')
    parser.add_argument('--archive', type=Path, help='import a downloaded release archive instead of using the network')
    parser.add_argument('--keep', action='store_true', help='retain the successfully verified extraction under .local for inspection')
    args = parser.parse_args()
    try:
        manifest = json.loads(args.manifest.read_text())
        cache = ROOT / '.local/benchmark-artifacts'
        archive = ensure_archive(manifest, cache, args.archive, args.offline)
        with tempfile.TemporaryDirectory(prefix='verify-', dir=cache) as directory:
            extracted = Path(directory)
            extract_archive(archive, extracted, manifest)
            verify_reports(extracted, manifest)
            if args.keep:
                retained = Path(tempfile.mkdtemp(prefix='verified-', dir=cache))
                retained.rmdir()
                extracted.rename(retained)
                print('Verified evidence retained at: ' + str(retained))
        print('PASS: archive integrity and all verifiers selected by the manifest; cache: ' + str(archive))
        return 0
    except (ValueError, OSError, EOFError, tarfile.TarError, subprocess.SubprocessError) as error:
        print('Evidence verification failed: ' + str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
