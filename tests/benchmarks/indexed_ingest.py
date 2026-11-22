# SPDX-License-Identifier: Apache-2.0
"""Matched synthetic HTTP ingestion; serialization is reported outside upload clocks."""
import hashlib,json,time
from comparison import corpus_value,corpus_digest,OracleMismatch
from elasticsearch import mapping

def select_spec(spec):
 layout=spec.get('byte_dispatch',{})
 if layout.get('strategy')!='byte_dispatch' or layout.get('fallback') is not None or len(layout.get('queries',[]))!=1:
  raise ValueError('byte_dispatch layout requires a supported one-query dispatch plan')
 return {**spec,'queries':list(layout['queries']),
         'diagnostics':dict(strategy='byte_dispatch',source_queries=1,estimated_ast=layout['estimated_ast'])}

def require(condition,message):
 if not condition:raise OracleMismatch(message)

def phases(spec,elastic):
 if not 0<spec['rows']<=1_000_000:raise ValueError('wire ingestion supports1..1000000documents')
 split=spec['rows']*9//10
 for label,first,stop in [('initial',0,split),('incremental',split,spec['rows'])]:
  start=time.monotonic();chunks=[]
  for offset in range(first,stop,2000):
   lines=[]
   for row in range(offset,min(offset+2000,stop)):
    if elastic:lines.append(json.dumps({'index':{'_id':str(row)}},separators=(',',':')))
    lines.append(json.dumps({'id':row,'v':corpus_value(spec,row)},separators=(',',':')))
   body='\n'.join(lines)+'\n'
   if not elastic:body='INSERT INTO bench FORMAT JSONEachRow\n'+body
   chunks.append((body,len(lines)//(2 if elastic else 1)))
  yield label,stop,chunks,time.monotonic()-start

def native_setup(engine,spec,out,*,emit,storage,source_hash,literal,durability='fsync'):
 if durability not in ('defaults','fsync'):raise ValueError('invalid ingestion durability')
 select_spec(spec)
 engine.query('CREATE TABLE templates(id UInt64,v String) ENGINE=MergeTree ORDER BY id')
 for start in range(0,len(spec['templates']),64):
  engine.query('INSERT INTO templates VALUES '+','.join(f'({i},{literal(v)})' for i,v in enumerate(spec['templates'][start:start+64],start)))
 create='CREATE TABLE bench(id Int64,v String) ENGINE=MergeTree ORDER BY id'
 if durability=='fsync':create+=' SETTINGS fsync_after_insert=1,fsync_part_directory=1'
 ddl=[create,spec['byte_dispatch']['index_ddl']]
 start=time.monotonic()
 for sql in ddl:engine.query(sql)
 emit(out,kind='wire_ddl',engine='native',durability=durability,seconds=time.monotonic()-start,ddl=ddl)
 confirmed=engine.query('SHOW CREATE TABLE bench')[0][0]
 (out/'native-confirmed-ddl.sql').write_text(confirmed+'\n')
 if durability=='fsync':require('fsync_after_insert = 1' in confirmed and 'fsync_part_directory = 1' in confirmed,'native fsync settings differ')
 for label,stop,chunks,prepare in phases(spec,False):
  start=time.monotonic()
  for body,count in chunks:engine.query(body)
  acknowledged=time.monotonic()-start
  visible=int(engine.query('SELECT count() FROM bench')[0][0]);elapsed=time.monotonic()-start
  require(visible==stop,'visible document count differs')
  emit(out,kind='wire_ingest',engine='native',phase=label,documents=sum(c for _,c in chunks),wire_bytes=sum(len(b.encode()) for b,_ in chunks),requests=len(chunks),client_prepare_seconds=prepare,upload_ack_seconds=acknowledged,searchable_seconds=elapsed,visible_rows=visible,contract='HTTP upload through count-confirmed searchable visibility; index maintained on insert; serialization excluded')
 start=time.monotonic();engine.query('OPTIMIZE TABLE bench FINAL');compact=time.monotonic()-start
 checksum=source_hash(engine,'bench');require(checksum==corpus_digest(spec),'native source corpus digest differs')
 emit(out,kind='wire_settled',engine='native',compaction_seconds=compact,storage=storage(engine,'bench'),corpus_sha256=checksum)
 return False

def elastic_hash(client,index,rows):
 digest=hashlib.sha256();after=None;seen=0
 while seen<rows:
  body={'size':10000,'sort':[{'id':'asc'}],'track_total_hits':False,'_source':['id','v'],'query':{'match_all':{}}}
  if after is not None:body['search_after']=after
  result=client.request('POST','/'+index+'/_search?request_cache=false',body)
  require(not result.get('timed_out',True) and result['_shards']['failed']==0,'Elastic source verification timed out or lost shards')
  hits=result['hits']['hits'];require(bool(hits),'Elastic source verification truncated')
  for hit in hits:
   row=hit['_source'];require(row['id']==seen,'Elastic source document sequence differs')
   digest.update(f"{row['id']}\t{row['v']}\n".encode());seen+=1
  after=hits[-1]['sort']
 require(seen==rows,'Elastic source document count differs')
 return digest.hexdigest()

def elastic_setup(client,index,kind,spec,out,*,emit,durability='fsync'):
 if kind!='wildcard' or durability not in ('defaults','fsync'):raise ValueError('invalid indexed ingestion configuration')
 ddl=mapping(kind,{'owner':index,'corpus_sha256':corpus_digest(spec),'pattern_sha256':spec['pattern_sha256']})
 if durability=='fsync':ddl['settings']['index.translog.durability']='request'
 start=time.monotonic();client.request('PUT','/'+index,ddl)
 emit(out,kind='wire_ddl',engine='elastic',durability=durability,seconds=time.monotonic()-start,ddl=ddl)
 confirmed=client.request('GET','/'+index+'/_settings?include_defaults=true&flat_settings=true')[index]
 (out/'elastic-confirmed-settings.json').write_text(json.dumps(confirmed,indent=2)+'\n')
 if durability=='fsync':require(confirmed['settings'].get('index.translog.durability','').lower()=='request','Elastic fsync setting differs')
 for label,stop,chunks,prepare in phases(spec,True):
  start=time.monotonic()
  for body,count in chunks:
   response=client.request('POST','/'+index+'/_bulk',body,True)
   require(not response.get('errors') and len(response['items'])==count,'Elastic bulk ingestion failed or lost items')
  acknowledged=time.monotonic()-start
  client.request('POST','/'+index+'/_refresh')
  visible=client.request('GET','/'+index+'/_count')['count'];elapsed=time.monotonic()-start
  require(visible==stop,'visible document count differs')
  emit(out,kind='wire_ingest',engine='elastic',phase=label,documents=sum(c for _,c in chunks),wire_bytes=sum(len(b.encode()) for b,_ in chunks),requests=len(chunks),client_prepare_seconds=prepare,upload_ack_seconds=acknowledged,searchable_seconds=elapsed,visible_rows=visible,contract='HTTP bulk upload plus refresh through count-confirmed searchable visibility; serialization excluded')
 start=time.monotonic();client.request('POST','/'+index+'/_forcemerge?max_num_segments=1');compact=time.monotonic()-start
 checksum=elastic_hash(client,index,spec['rows']);require(checksum==corpus_digest(spec),'Elastic source corpus digest differs')
 emit(out,kind='wire_settled',engine='elastic',compaction_seconds=compact,storage=client.request('GET','/'+index+'/_stats/store,docs,segments'),corpus_sha256=checksum)
