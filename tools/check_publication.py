#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Check public documentation links and keep local archives out of releases."""
from pathlib import Path
from urllib.parse import unquote, urlsplit
import os
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
PRIVATE = {'.git', '.jj', '.local', '.agents', '.codex', '.aws', '.work', '.superpowers'}
GENERATED = {'target', '__pycache__', '.venv', '.pytest_cache', '.ruff_cache'}
MARKDOWN_LINK = re.compile(r'\]\((<[^>]+>|[^\s)]+)(?:\s+"[^"]*")?\)')


def public_files():
    for directory, children, files in os.walk(ROOT, followlinks=False):
        parent = Path(directory)
        children[:] = [name for name in children
                       if name not in PRIVATE | GENERATED
                       and not name.startswith('wc_iceberg_')
                       and parent / name not in {ROOT / 'fuzz/corpus', ROOT / 'fuzz/artifacts'}]
        for name in files:
            if name.endswith(('.pyc', '.pyo', '.log')) or name == '.DS_Store':
                continue
            yield parent / name


def main():
    problems = []
    if '/.local/' not in (ROOT / '.gitignore').read_text().splitlines():
        problems.append('.gitignore must exclude /.local/')
    documents = 0
    for path in public_files():
        relative = path.relative_to(ROOT)
        if path.suffix.lower() == '.pdf':
            problems.append(f'{relative}: move reports to .local/')
        if path.suffix not in {'.md', '.toml', '.yml', '.yaml'}:
            continue
        text = path.read_text()
        if 'github.com/your-org/' in text or re.search(r'/(?:home|Users)/[^/\s]+/', text):
            problems.append(f'{relative}: placeholder repository or private machine path')
        if path.suffix != '.md':
            continue
        documents += 1
        for target in MARKDOWN_LINK.findall(text):
            target = target.strip('<>')
            parts = urlsplit(target)
            if parts.scheme or parts.netloc or not parts.path:
                continue
            resolved = (path.parent / unquote(parts.path)).resolve()
            if not resolved.is_relative_to(ROOT):
                problems.append(f'{relative}: link leaves the repository: {target}')
            elif any(part in PRIVATE | GENERATED for part in resolved.relative_to(ROOT).parts):
                problems.append(f'{relative}: link points to private/generated content: {target}')
            elif not resolved.exists():
                problems.append(f'{relative}: missing link target: {target}')
    if problems:
        print('\n'.join(problems), file=sys.stderr)
        return 1
    print(f'Publication checks passed: {documents} public Markdown files; no broken local links or public PDFs.')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
