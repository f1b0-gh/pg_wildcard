#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Limit committed benchmark result files and their added/modified blobs per PR."""
import argparse
from pathlib import Path
import subprocess
import sys

FILE_LIMIT = 256 * 1024
PR_LIMIT = 2 * 1024 * 1024
RESULTS_PREFIX = b'tests/benchmarks/results/'


def git(repo, *arguments):
    return subprocess.check_output(['git', '-C', str(repo), *arguments])


def commit(repo, revision):
    return git(repo, 'rev-parse', '--verify', '--end-of-options', revision + '^{commit}').decode().strip()


def check(repo, head='HEAD', base=None):
    head = commit(repo, head)
    files = {}
    for record in git(repo, 'ls-tree', '-rlz', head).split(b'\0'):
        if record:
            metadata, path = record.split(b'\t', 1)
            _, kind, _, size = metadata.split()
            if kind == b'blob':
                files[path] = int(size)
    problems = []
    for path, size in files.items():
        if path.startswith(RESULTS_PREFIX) and size > FILE_LIMIT:
            problems.append(f'{path.decode(errors="replace")!r}: {size} bytes exceeds {FILE_LIMIT}; publish a release asset')
    if base is not None:
        base = commit(repo, base)
        changed = git(repo, 'diff', '--name-only', '--no-renames', '--diff-filter=ACMT', '-z', base, head, '--')
        total = sum(files.get(path, 0) for path in changed.split(b'\0')
                    if path.startswith(RESULTS_PREFIX))
        if total > PR_LIMIT:
            problems.append(f'PR added/modified benchmark result blobs total {total} bytes exceeds {PR_LIMIT}; deletions do not offset additions')
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--head', default='HEAD', help='PR head commit, not the synthetic merge commit')
    parser.add_argument('--base', help='PR merge base; omit outside a pull request')
    args = parser.parse_args()
    problems = check(Path.cwd(), args.head, args.base)
    if problems:
        print('\n'.join(problems), file=sys.stderr)
        return 1
    print('Artifact size policy passed' + (' (including benchmark results PR blob budget)' if args.base else ''))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
