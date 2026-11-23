# SPDX-License-Identifier: Apache-2.0
"""Offline evidence verification; no server or executable worker is needed."""
import gzip,hashlib,json,math,statistics,tomllib,re
from pathlib import Path
B=Path(__file__).resolve().parent

def require(ok,message):
 if not ok:raise ValueError(message)
def read(path):
 p=B/path;raw=p.read_bytes();return json.loads(gzip.decompress(raw) if p.suffix=='.gz' else raw)
def median(values):return statistics.median(values)
def expected(spec):
 pairs=[]
 for row in range(0,spec['rows'],spec['stride']):
  pairs.extend((row,int(indicator),view) for indicator,view in spec['template_hits'][(row//spec['stride'])%len(spec['templates'])])
 digest=hashlib.sha256()
 for row,indicator,view in sorted(pairs):digest.update(f'{row}\t{indicator}\t{view}\n'.encode())
 return len(pairs),digest.hexdigest()
manifest=read('sha256.json')
require({str(p.relative_to(B)) for p in B.rglob('*') if p.is_file()}==set(manifest)|{'sha256.json'},'payload membership')
for name,sha in manifest.items():
 p=Path(name);require(not p.is_absolute() and '..' not in p.parts,'unsafe manifest path')
 require(hashlib.sha256((B/p).read_bytes()).hexdigest()==sha,'payload hash '+name)
locks={v:tomllib.loads((B/'sources'/v/'Cargo.lock').read_text())['package'] for v in ('0.1.0','0.1.1')}
for v,checksum in [('0.1.0','3bc1afbdfb6d217b4670615038ba391a65f5cf422378213c727470dfb1d7fa09'),('0.1.1','7e62072e6ded6a106097979df75e37d70f02ab61d2b4a45db5afdf0421e24ebd')]:
 package=next(p for p in locks[v] if p['name']=='antalya-rs')
 require(package['version']==v and package['checksum']==checksum and package['source']=='registry+https://github.com/rust-lang/crates.io-index','published release identity')
require([p for p in locks['0.1.0'] if p['name']!='antalya-rs' and not p['name'].startswith('sweep-release-')]==[p for p in locks['0.1.1'] if p['name']!='antalya-rs' and not p['name'].startswith('sweep-release-')],'transitive dependencies differ')
require((B/'sources/0.1.0/main.rs').read_bytes()==(B/'sources/0.1.1/main.rs').read_bytes(),'worker source differs')
ORIGINAL_SPEC_SHA={'shared': 'aff51a71d1a9001ebea5d0c066dc872cc8e796d3c0f50c41e8ed53d0b4a29e52', 'unrelated': '28fb5b4050496fabf8af86f5b7596a1a9e73d0b6161b343716622032bb630a4b'}
corpora={}
seen=set();count=0
for route in ('direct','postgres'):
 summary=read(route+'/summary.json')
 variants=['010-none','011-none','011-zstd'] if route=='direct' else ['backend-none','backend-zstd','copy-none','copy-zstd']
 for workload in ('shared','unrelated'):
  spec=read('specs/'+workload+'.json.gz');n,sha=expected(spec);require(n==10000,'oracle pair count')
  require(spec['rows']==1000000 and spec['indicators']==1000,'workload shape')
  if workload not in corpora:
   h=hashlib.sha256()
   for row in range(spec['rows']):
    value=spec['templates'][(row//spec['stride'])%len(spec['templates'])] if row%spec['stride']==0 else spec['benign_prefix']+str(row)+spec.get('benign_suffix','')
    h.update(f'{row}\t{value}\n'.encode())
   corpora[workload]=h.hexdigest()
  identity=read(f'{route}/{workload}/identity.json')
  require(identity['corpus_sha256']==corpora[workload],'source corpus digest')
  require(identity['spec_sha256']==ORIGINAL_SPEC_SHA[workload],'original November 22 specification')
  queries=read(f'{route}/{workload}/queries.json.gz')
  if route=='postgres':queries=queries['native']
  normalize=lambda q:re.sub(r'wc_benchmark_[a-f0-9]+\.bench','bench',q)
  require(list(map(normalize,queries))==spec['byte_dispatch']['queries'],'original generated SQL')
  require(identity['pattern_sha256']==spec['pattern_sha256'],'rule identity')
  require(identity['spec_sha256']==hashlib.sha256(gzip.decompress((B/f'specs/{workload}.json.gz').read_bytes())).hexdigest(),'spec identity')
  raw=gzip.decompress((B/f'{route}/{workload}/raw.jsonl.gz').read_bytes())
  records=[json.loads(line) for line in raw.splitlines()]
  settled=[r for r in records if r['kind']=='wire_settled'];require(len(settled)==1 and settled[0]['corpus_sha256']==corpora[workload],'engine corpus verification receipt')
  samples=[r for r in records if r['kind']=='sample'];profiles=[r for r in records if r['kind']=='profile']
  require(len(samples)==len(variants)*7 and len(profiles)==len(samples),'complete sample/profile cohort')
  for variant in variants:
   rs=[r for r in samples if r['variant']==variant]
   require(sorted(r['repeat'] for r in rs)==list(range(-1,6)),'repeat membership')
   for r in rs:
    require(r['warmup']==(r['repeat']==-1),'warmup flag')
    require(math.isfinite(r['request_ms']) and 0<r['request_ms']<60000,'request clock')
    if route=='direct' or variant.startswith('copy'):
     require(r['pairs']==n and r['pair_sha256']==sha,'exact oracle receipt')
    else:
     node=r['plan']['Plan'];require(node['Node Type']=='Foreign Scan','PG plan shape')
     require(node['Actual Rows']*node['Actual Loops']==n,'PG backend row count')
     require(node['FDW metrics complete'] is True and node['FDW remote rows']==n,'complete PG metrics')
     require('full COPY precontrol' in r['verification'],'PG correctness scope')
    ps=[p for p in profiles if p['variant']==variant and p['repeat']==r['repeat']]
    require(len(ps)==1,'unique profile');p=ps[0];qid=p['query_id'];require(qid not in seen,'query ID reused');seen.add(qid)
    if route=='direct':
     require(len(r['executions'])==1 and r['executions'][0]['query_id']==qid,'direct correlation')
     require(r['executions'][0]['peak_individual_batch_bytes']<=16<<20,'output batch bound')
    elif variant.startswith('backend'):require(r['query_ids']==[qid],'PG backend correlation')
    profile=p['profile'];codec='zstd' if variant.endswith('zstd') else 'none'
    require(profile['settings']['output_format_arrow_compression_method']==codec,'server codec')
    require(profile['settings']['max_threads']=='4','engine parallelism')
    require(int(profile['result_rows'])==n,'engine pair count')
    require(profile['profile_events'].get('TextIndexReadPostings',0)>0,'text index used')
    require(profile['profile_events'].get('QueryConditionCacheHits',0)==0,'condition cache hit')
    count+=1
   rs=[r for r in rs if not r['warmup']]
   actual=summary[workload][variant]
   if route=='direct':
    for key in ('request_ms','worker_roundtrip_ms'):require(actual[key]==median([r[key] for r in rs]),'direct median '+key)
    for key in ('wire_bytes','decode_ms','body_wait_ms','terminal_ms'):
     require(actual[key]==median([sum(e[key] or 0 for e in r['executions']) for r in rs]),'direct metric '+key)
    require(actual['request_samples_ms']==[r['request_ms'] for r in rs],'direct sample vector')
   else:
    key='executor_ms' if variant.startswith('backend') else 'request_ms'
    require(actual['clock']==key and actual['samples_ms']==[r[key] for r in rs],'PG clocks/samples')
    require(actual['median_ms']==median([r[key] for r in rs]),'PG median')
  if route=='direct':
   cs=[r for r in records if r['kind']=='canary'];require(len(cs)==3 and all(r['pairs']==1000 for r in cs),'all-rule direct canaries')
  else:
   cs=[r for r in records if r['kind']=='precontrol'];require(len(cs)==4,'PG precontrols')
   require(all(r['pair_sha256']==sha and r['pairs']==n for r in cs if r['target']=='bench'),'PG exact full-copy precontrols')
cleanup=read('cleanup.json');require(cleanup['native_removed'] and cleanup['pg_restored'],'fixture restoration')
print(f'PASS: hashes, {count} samples with oracle receipts and terminal correlation, codecs, index use, medians and fixture restoration')
