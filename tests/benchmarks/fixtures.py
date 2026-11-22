#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Create/remove only owned, isolated benchmark engines from locally cached images.

Private state (including a synthetic JWT) belongs under .local/, never in results.
No existing service is used and images are never pulled implicitly.
"""
import argparse, base64, hashlib, hmac, json, os, secrets, subprocess, time
from pathlib import Path
import urllib.request

IMAGES = {
    'antalya': 'altinityinfra/clickhouse-server@sha256:0989fbcd21552463cff9ef8d7a44bd2213a2c8e1879816f856a744a14b28e77a',
    'elasticsearch': 'docker.elastic.co/elasticsearch/elasticsearch@sha256:79b4f1de4c15e6eca824723086a51e5934406d614d410cc11be77748cafac537',
}

def command(*args, **kw):
    return subprocess.run(args, text=True, capture_output=True, check=True, **kw).stdout.strip()

def token(key):
    enc = lambda b: base64.urlsafe_b64encode(b).rstrip(b'=')
    body = b'.'.join(enc(json.dumps(v, separators=(',', ':')).encode()) for v in [
        {'alg': 'HS256', 'typ': 'JWT'},
        {'sub': 'benchmark', 'iss': 'wildcard-benchmark', 'aud': 'wildcard-benchmark', 'exp': int(time.time()) + 86400}])
    return (body + b'.' + enc(hmac.new(key.encode(), body, hashlib.sha256).digest())).decode()

def start(engine, state):
    if state.exists():
        raise RuntimeError('State already exists; stop its owned fixture first')
    state.parent.mkdir(parents=True, exist_ok=True)
    state.parent.chmod(0o700)
    owner = secrets.token_hex(8)
    name = 'wildcard-benchmark-' + engine + '-' + owner
    network = name + '-net'
    data = {'engine': engine, 'name': name, 'network': network, 'owner': owner,
            'image': IMAGES[engine], 'cpus': 4, 'memory_bytes': 8 << 30 if engine == 'antalya' else 4 << 30}
    command('docker', 'image', 'inspect', data['image'])
    command('docker', 'network', 'create', '--internal', '--label', 'wildcard.benchmark.owner=' + owner, network)
    # Save ownership before startup so a failed startup can be cleaned safely.
    state.write_text(json.dumps(data));state.chmod(0o600)
    args = ['docker', 'run', '-d', '--pull', 'never', '--name', name, '--label', 'wildcard.benchmark.owner=' + owner,
            '--network', network, '--cpus', '4', '--memory', str(data['memory_bytes'])]
    if engine == 'antalya':
        secret = secrets.token_hex(32)
        config = state.with_suffix('.xml')
        config.write_text(f'''<clickhouse>
<enable_token_auth>1</enable_token_auth>
<token_processors><benchmark><type>jwt_static_key</type><algo>HS256</algo><static_key>{secret}</static_key>
<username_claim>sub</username_claim><expected_issuer>wildcard-benchmark</expected_issuer><expected_audience>wildcard-benchmark</expected_audience>
<allow_no_expiration>false</allow_no_expiration><token_cache_lifetime>0</token_cache_lifetime></benchmark></token_processors>
<max_server_memory_usage>7000000000</max_server_memory_usage>
<background_schedule_pool_size>8</background_schedule_pool_size><background_message_broker_schedule_pool_size>2</background_message_broker_schedule_pool_size>
<background_distributed_schedule_pool_size>2</background_distributed_schedule_pool_size>
<query_log><flush_interval_milliseconds>1000</flush_interval_milliseconds></query_log>
</clickhouse>''')
        config.chmod(0o644)  # parent .local directory controls access; daemon must read it.
        share = state.parent / 'shared';share.mkdir(exist_ok=True);share.chmod(0o777)
        args += ['-v', f'{config.resolve()}:/etc/clickhouse-server/config.d/benchmark.xml:ro',
                 '-v', f'{share.resolve()}:/var/lib/clickhouse/user_files/benchmark:rw']
        data['token'] = token(secret)
        data['share_dir'] = str(share.resolve())
        port = 8123
    else:
        args += ['-e', 'discovery.type=single-node', '-e', 'xpack.security.enabled=false',
                 '-e', 'ES_JAVA_OPTS=-Xms2g -Xmx2g', '-e', 'node.processors=4']
        port = 9200
    command(*args, data['image'])
    info = json.loads(command('docker', 'inspect', name))[0]
    address = info['NetworkSettings']['Networks'][network]['IPAddress']
    data['url'] = f'http://{address}:{port}'
    state.write_text(json.dumps(data));state.chmod(0o600)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    for _ in range(90):
        try:
            if engine == 'antalya':
                version = command('docker', 'exec', name, 'clickhouse-client', '-q', 'SELECT version()')
                command('docker', 'exec', name, 'clickhouse-client', '--multiquery', '-q',
                        "CREATE USER benchmark IDENTIFIED WITH jwt SETTINGS max_threads=4, max_memory_usage=4000000000, log_queries=1, log_query_settings=1; GRANT FILE, SELECT, INSERT, CREATE DATABASE, CREATE TABLE, CREATE VIEW, DROP DATABASE, DROP TABLE, DROP VIEW, ALTER TABLE, TRUNCATE, OPTIMIZE, KILL QUERY, SYSTEM FLUSH LOGS ON *.* TO benchmark;")
            else:
                version = json.load(opener.open(data['url'], timeout=2))['version']['number']
                # Owned disposable cluster: keep absolute free-space guards on
                # shared development disks whose percentage use may exceed 85%.
                disk = {'persistent': {'cluster.routing.allocation.disk.watermark.low': '10gb',
                    'cluster.routing.allocation.disk.watermark.high': '5gb',
                    'cluster.routing.allocation.disk.watermark.flood_stage': '2gb'}}
                request = urllib.request.Request(data['url']+'/_cluster/settings', data=json.dumps(disk).encode(), method='PUT', headers={'Content-Type':'application/json'})
                with opener.open(request, timeout=5) as response: json.load(response)
                data['disk_watermarks'] = disk['persistent']
            data['version'] = version;state.write_text(json.dumps(data));state.chmod(0o600)
            print(json.dumps({k: data[k] for k in ['engine', 'name', 'image', 'version', 'cpus', 'memory_bytes']}))
            return
        except (subprocess.CalledProcessError, OSError, ValueError):
            time.sleep(1)
    raise RuntimeError('Engine did not become ready; private state retained for stop')

def stop(state):
    data = json.loads(state.read_text())
    for kind, name in [('container', data['name']), ('network', data['network'])]:
        probe = subprocess.run(['docker', kind, 'inspect', name], capture_output=True, text=True)
        if probe.returncode:
            continue
        info = json.loads(probe.stdout)[0]
        labels = info['Config']['Labels'] if kind == 'container' else info['Labels']
        if labels.get('wildcard.benchmark.owner') != data['owner']:
            raise RuntimeError('Ownership mismatch; refusing cleanup')
        if kind == 'container':command('docker', 'rm', '-fv', name)
        else:command('docker', 'network', 'rm', name)
    state.unlink();state.with_suffix('.xml').unlink(missing_ok=True)

if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action', choices=['start', 'stop']);p.add_argument('--engine', choices=IMAGES)
    p.add_argument('--state', type=Path, required=True);a = p.parse_args()
    if a.action == 'start':
        if not a.engine:p.error('--engine is required for start')
        start(a.engine, a.state)
    else:stop(a.state)
