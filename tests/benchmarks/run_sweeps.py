#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Run all native sweep/ingest variants, preserving failures and timeout evidence."""
import argparse
import datetime
import json
import re
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path


def cleanup(state_path, log):
    """Only cancel/drop the unique databases identified by this child's query IDs."""
    ids = set()
    for line in log.read_text().splitlines():
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if record.get('kind') == 'owned_database':
            database = record.get('database', '')
            if re.fullmatch(r'wc_benchmark_[0-9a-f]{32}', database):
                ids.add(database)
        query_id = record.get('query_id', '')
        match = re.fullmatch(r'(wc_benchmark_[0-9a-f]{32})_[0-9a-f]{32}', query_id)
        if match:
            ids.add(match[1])
    state = json.loads(state_path.read_text())
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    for database in sorted(ids):
        for sql in [f"KILL QUERY WHERE startsWith(query_id,'{database}_') SYNC",
                    f'DROP DATABASE IF EXISTS {database}']:
            request = urllib.request.Request(
                state['url'] + '/?' + urllib.parse.urlencode({'max_execution_time': 30}),
                data=sql.encode(), headers={'Authorization': 'Bearer ' + state['token']})
            with opener.open(request, timeout=60) as response:
                response.read()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('spec', type=Path)
    parser.add_argument('--state', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--timeout', type=int, default=1000,
                        help='whole variant wall limit, including setup and all repetitions')
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    spec = json.loads(args.spec.read_text())
    runner = Path(__file__).resolve().parents[2] / 'libwildcard/tests/benchmarks/sweeps.py'
    variants = [('sweeps', s) for s in spec['queries']]
    variants += [('ingest', s) for s in spec['watchlist']['mvs']]
    variants += [('candidates', 'all')]
    statuses = []
    for mode, strategy in variants:
        log = args.out / f'{mode}-{strategy}.jsonl'
        started = time.monotonic()
        status = dict(mode=mode, strategy=strategy,
                      started_utc=datetime.datetime.now(datetime.timezone.utc).isoformat())
        with args.spec.open() as source, log.open('w') as output:
            try:
                child = subprocess.run(
                    [sys.executable, str(runner), '--state', str(args.state), '--mode', mode,
                     '--strategy', strategy, '--repeats', str(args.repeats)],
                    stdin=source, stdout=output, stderr=subprocess.STDOUT, timeout=args.timeout)
                status['exit_code'] = child.returncode
                status['status'] = 'completed' if child.returncode == 0 else 'failed'
            except subprocess.TimeoutExpired:
                status.update(status='censored_process_timeout', timeout_seconds=args.timeout)
        status['invocation_wall_seconds'] = time.monotonic() - started
        statuses.append(status)
        status_file = args.out / 'run-status.json'
        status_file.write_text(json.dumps(statuses, indent=2) + '\n')
        # A failed/censored measurement must leave no queries running into the next one.
        try:
            cleanup(args.state, log)
            status['cleanup'] = 'confirmed'
        except Exception as error:
            status['cleanup'] = 'failed'
            message = str(error)
            private = json.loads(args.state.read_text())
            for key in ['token', 'url']:
                if private.get(key):
                    message = message.replace(private[key], '[redacted]')
            status['cleanup_error'] = message
            status_file.write_text(json.dumps(statuses, indent=2) + '\n')
            raise SystemExit('Cleanup failed; see sanitized run-status.json. Stopping before the next variant.') from None
        status_file.write_text(json.dumps(statuses, indent=2) + '\n')
        print(json.dumps(status), flush=True)
    return 1 if any(s['status'] != 'completed' for s in statuses) else 0


if __name__ == '__main__':
    raise SystemExit(main())
