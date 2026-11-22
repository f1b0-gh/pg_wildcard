import hashlib,json,os,signal,subprocess,sys,time
from pathlib import Path
sys.path.insert(0,'tests/benchmarks')
from comparison import expected_pairs
root=Path('.local/perf');(root/'matrix').mkdir(exist_ok=True)
gen=root/'execution/sweep-benchmark-baseline'
worker=root.parent/'review-pg/target/release/examples/sweep_route'
cases=[(variant,count,1) for count in [100,1000] for variant in ['shared','unrelated','common','short','mixed']]
cases += [(variant,1000,4) for variant in ['shared','unrelated','common']]
cases += [('domains-current',100,1)]
cases += [(variant,10000,1) for variant in ['shared','unrelated','common','short','mixed']]
status=json.loads((root/'execution/matrix-status.json').read_text()) if (root/'execution/matrix-status.json').exists() else []
for variant,count,concurrency in cases:
    name=f'{variant}-{count}-c{concurrency}'
    if any(r['case']==name and r['status'] in ('complete','censored_pair_budget','censored_generation_budget') for r in status):continue
    spec_path=(root/'anchored-needles/specs/domains-100-1000000.json' if variant=='domains-current' else root/'specs'/f'{variant}-{count}.json')
    if variant=='domains-current' and not spec_path.exists():raise RuntimeError('current domain renderer specification missing')
    if not spec_path.exists():
        try:
            result=subprocess.run([str(gen),'--comparison',variant,'--indicators',str(count),'--rows','1000000'],capture_output=True,timeout=60)
        except subprocess.TimeoutExpired:
            status.append({'case':name,'status':'censored_generation_budget','generator_timeout_seconds':60,'reason':'bounded stress fixture generation did not finish; no query timing or correctness claim'})
            (root/'execution/matrix-status.json').write_text(json.dumps(status,indent=2)+'\n');continue
        if result.returncode:raise RuntimeError('generator failed: '+result.stderr.decode()[-2000:])
        spec_path.write_bytes(result.stdout)
    spec=json.loads(spec_path.read_text())
    try:expected_pairs(spec)
    except ValueError as error:
        if str(error)!='oracle exceeds pair budget':raise
        status.append({'case':name,'status':'censored_pair_budget','error':str(error),'spec_sha256':hashlib.sha256(spec_path.read_bytes()).hexdigest()})
        (root/'execution/matrix-status.json').write_text(json.dumps(status,indent=2)+'\n');continue
    # New admission metadata declares the synthetic v field; preserve original evidence.
    spec['schema']='wildcard-comparison-v2'
    if variant!='domains-current':spec['search_target']={'kind':'field','field':'v','data_type':'wildcard','synthetic':True}
    scoped=root/'scoped-specs'/f'{variant}-{count}-matrix.json'
    scoped.write_text(json.dumps(spec,indent=2)+'\n')
    spec_path=scoped
    out=root/'matrix'/name
    if out.exists():raise RuntimeError('refusing to overwrite case '+name)
    # The keyword layout is an explicit historical storage control, not an
    # eligible keyword field in the application's wildcard-search policy.
    command=['python3','tests/benchmarks/compare_routes.py',str(spec_path),'--native-state',str(root/'antalya.json'),'--elastic-state',str(root/'elasticsearch.json'),'--pg-container','wildcard-review-pg18','--direct-worker',str(worker),'--out',str(out),'--routes','native,indexed,direct,pg_backend,pg_copy,elastic_wildcard,elastic_keyword','--repeats','5','--concurrency',str(concurrency)]
    print('START '+name,flush=True)
    marker=root/'execution/timing-active'
    if marker.exists():raise RuntimeError('another timing window is active')
    marker.write_text(name+'\n')
    start=time.monotonic()
    with (root/'execution'/('matrix-'+name+'.log')).open('w') as log:
        process=subprocess.Popen(command,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
        try:result=process.wait(timeout=2700)
        except BaseException:
            os.killpg(process.pid,signal.SIGTERM)
            try:process.wait(timeout=30)
            except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
            raise
        finally:marker.unlink(missing_ok=True)
    status.append({'case':name,'status':'complete' if result==0 else 'failed','exit_status':result,'seconds':time.monotonic()-start})
    (root/'execution/matrix-status.json').write_text(json.dumps(status,indent=2)+'\n')
    print('END '+name+' status='+str(result),flush=True)
    if result:raise SystemExit('case failed; stop to inspect before further timings')
print('MATRIX COMPLETE',flush=True)
