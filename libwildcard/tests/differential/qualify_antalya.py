#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Run differential specifications against an owned, disposable Antalya server.

Requires an existing local pinned image; never downloads or touches another server.
"""
import argparse
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import time

IMAGE = "altinity/clickhouse-server@sha256:aec9534624af9142abe87aab1b51ff5bcca9e74ca3b7e58fead2c1ec5e157cdf"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("generator", type=Path)
    parser.add_argument("--seeds", nargs="+", type=int, default=[1, 42, 1234])
    args = parser.parse_args()
    parent = Path("/tmp/libwildcard")
    parent.mkdir(exist_ok=True)
    if parent.is_symlink() or parent.stat().st_uid != os.getuid():
        raise ValueError("unsafe temporary directory")
    container_id = None
    with tempfile.TemporaryDirectory(prefix="antalya-", dir=parent) as work:
        config = Path(work) / "qualification.xml"
        config.write_text("""<clickhouse>
<max_server_memory_usage>800000000</max_server_memory_usage>
<background_schedule_pool_size>8</background_schedule_pool_size>
<background_message_broker_schedule_pool_size>2</background_message_broker_schedule_pool_size>
<background_distributed_schedule_pool_size>2</background_distributed_schedule_pool_size>
<mark_cache_size>16777216</mark_cache_size><uncompressed_cache_size>16777216</uncompressed_cache_size>
</clickhouse>""")
        name = "soc-http-test-libwildcard-" + secrets.token_hex(6) + "-ch"
        try:
            container_id = subprocess.check_output([
                "docker", "run", "--pull=never", "-d", "--name", name,
                "--label", "libwildcard.qualification=true", "--memory", "1g", "--cpus", "2",
                "-v", f"{config}:/etc/clickhouse-server/config.d/qualification.xml:ro", IMAGE,
            ], text=True).strip()
            for _ in range(60):
                check = subprocess.run(["docker", "exec", name, "clickhouse-client", "-q", "SELECT version()"], capture_output=True, text=True)
                if check.returncode == 0:
                    if check.stdout.strip() != "26.6.8.20004.altinityantalya":
                        raise ValueError("unexpected Antalya version")
                    break
                time.sleep(1)
            else:
                raise RuntimeError("Antalya did not start")
            for seed in args.seeds:
                print("Antalya seed", seed, flush=True)
                spec = subprocess.check_output([str(args.generator.resolve()), str(seed)])
                subprocess.run([sys.executable, str(Path(__file__).with_name("run.py")), "--antalya-container", name], input=spec, check=True)
        finally:
            if container_id:
                subprocess.run(["docker", "rm", "-f", container_id], check=True, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
