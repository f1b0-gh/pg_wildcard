"""Change only the owned fixture's log cadence; restore its configuration on exit."""
import argparse,json,os,subprocess,time,urllib.request,xml.etree.ElementTree as ET
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--state',type=Path,required=True);p.add_argument('--interval',type=int,choices=(100,1000),required=True);p.add_argument('--out',type=Path,required=True);p.add_argument('command',nargs=argparse.REMAINDER);a=p.parse_args()
state=json.loads(a.state.read_text());info=json.loads(subprocess.check_output(['docker','inspect',state['name']],text=True))[0]
if info['Config']['Labels'].get('wildcard.benchmark.owner')!=state['owner']:raise ValueError('not owned')
if not info['State']['Running']:raise ValueError('owned fixture was not running')
if a.command[:1]==['--']:a.command=a.command[1:]
config=Path(state['artifact_dir'])/'benchmark.xml';original=config.read_bytes();a.out.mkdir(parents=True,exist_ok=False)
root=ET.fromstring(original);field=root.find('query_log/flush_interval_milliseconds');before=int(field.text)
if not a.command:raise ValueError('missing benchmark command')
opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
def restart():
    subprocess.run(['docker','restart',state['name']],check=True,stdout=subprocess.DEVNULL,timeout=60)
    for _ in range(60):
        try:
            request=urllib.request.Request(state['url'],data=b'SELECT 1',headers={'Authorization':'Bearer '+state['token']})
            with opener.open(request,timeout=2) as response:
                if response.read()==b'1\n':return
        except Exception:pass
        time.sleep(1)
    raise RuntimeError('owned server did not become ready')
receipt=dict(original_interval_ms=before,requested_interval_ms=a.interval,original_config_sha256=__import__('hashlib').sha256(original).hexdigest())
try:
    field.text=str(a.interval)
    # Keep the bind-mounted inode: replacing a file would leave Docker on the old inode.
    config.write_bytes(ET.tostring(root));restart()
    receipt['started_at']=json.loads(subprocess.check_output(['docker','inspect',state['name']],text=True))[0]['State']['StartedAt']
    with (a.out/'run.log').open('w') as log:
        result=subprocess.run(a.command,stdout=log,stderr=subprocess.STDOUT)
    receipt['exit_status']=result.returncode
finally:
    try:
        config.write_bytes(original)
        restart()
        receipt['restored']=config.read_bytes()==original
    finally:
        (a.out/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
if receipt.get('exit_status')!=0:raise SystemExit(receipt.get('exit_status',1))
