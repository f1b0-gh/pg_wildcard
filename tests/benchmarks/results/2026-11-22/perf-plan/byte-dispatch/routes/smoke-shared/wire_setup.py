"""Matched synthetic HTTP ingestion; serialization is reported outside upload clocks."""
import hashlib,json,time
from compare_routes import emit,storage,source_hash,sweeps
from comparison import corpus_value,corpus_digest
from elasticsearch import mapping

def phases(spec,elastic):
 assert 0<spec['rows']<=1_000_000
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

def native_setup(engine,spec,out,indexed):
 assert not indexed and spec['byte_dispatch']['strategy']=='byte_dispatch'
 engine.query('CREATE TABLE templates(id UInt64,v String) ENGINE=MergeTree ORDER BY id')
 for start in range(0,len(spec['templates']),64):
  engine.query('INSERT INTO templates VALUES '+','.join(f'({i},{sweeps.literal(v)})' for i,v in enumerate(spec['templates'][start:start+64],start)))
 ddl=['CREATE TABLE bench(id Int64,v String) ENGINE=MergeTree ORDER BY id',spec['byte_dispatch']['index_ddl']]
 start=time.monotonic()
 for sql in ddl:engine.query(sql)
 emit(out,kind='wire_ddl',engine='native',seconds=time.monotonic()-start,ddl=ddl)
 for label,stop,chunks,prepare in phases(spec,False):
  start=time.monotonic()
  for body,count in chunks:engine.query(body)
  acknowledged=time.monotonic()-start
  visible=int(engine.query('SELECT count() FROM bench')[0][0]);elapsed=time.monotonic()-start
  assert visible==stop
  emit(out,kind='wire_ingest',engine='native',phase=label,documents=sum(c for _,c in chunks),wire_bytes=sum(len(b.encode()) for b,_ in chunks),requests=len(chunks),client_prepare_seconds=prepare,upload_ack_seconds=acknowledged,searchable_seconds=elapsed,visible_rows=visible,contract='HTTP upload through count-confirmed searchable visibility; index maintained on insert; serialization excluded')
 start=time.monotonic();engine.query('OPTIMIZE TABLE bench FINAL');compact=time.monotonic()-start
 checksum=source_hash(engine,'bench');assert checksum==corpus_digest(spec)
 emit(out,kind='wire_settled',engine='native',compaction_seconds=compact,storage=storage(engine,'bench'),corpus_sha256=checksum)
 return False

def elastic_hash(client,index,rows):
 digest=hashlib.sha256();after=None;seen=0
 while seen<rows:
  body={'size':10000,'sort':[{'id':'asc'}],'track_total_hits':False,'_source':['id','v'],'query':{'match_all':{}}}
  if after is not None:body['search_after']=after
  result=client.request('POST','/'+index+'/_search?request_cache=false',body)
  assert not result.get('timed_out',True) and result['_shards']['failed']==0
  hits=result['hits']['hits'];assert hits
  for hit in hits:
   row=hit['_source'];assert row['id']==seen
   digest.update(f"{row['id']}\t{row['v']}\n".encode());seen+=1
  after=hits[-1]['sort']
 assert seen==rows
 return digest.hexdigest()

def elastic_setup(client,index,kind,spec,out):
 assert kind=='wildcard'
 ddl=mapping(kind,{'owner':index,'corpus_sha256':corpus_digest(spec),'pattern_sha256':spec['pattern_sha256']})
 start=time.monotonic();client.request('PUT','/'+index,ddl)
 emit(out,kind='wire_ddl',engine='elastic',seconds=time.monotonic()-start,ddl=ddl)
 for label,stop,chunks,prepare in phases(spec,True):
  start=time.monotonic()
  for body,count in chunks:
   response=client.request('POST','/'+index+'/_bulk',body,True)
   assert not response.get('errors') and len(response['items'])==count
  acknowledged=time.monotonic()-start
  client.request('POST','/'+index+'/_refresh')
  visible=client.request('GET','/'+index+'/_count')['count'];elapsed=time.monotonic()-start
  assert visible==stop
  emit(out,kind='wire_ingest',engine='elastic',phase=label,documents=sum(c for _,c in chunks),wire_bytes=sum(len(b.encode()) for b,_ in chunks),requests=len(chunks),client_prepare_seconds=prepare,upload_ack_seconds=acknowledged,searchable_seconds=elapsed,visible_rows=visible,contract='HTTP bulk upload plus refresh through count-confirmed searchable visibility; serialization excluded')
 start=time.monotonic();client.request('POST','/'+index+'/_forcemerge?max_num_segments=1');compact=time.monotonic()-start
 checksum=elastic_hash(client,index,spec['rows']);assert checksum==corpus_digest(spec)
 emit(out,kind='wire_settled',engine='elastic',compaction_seconds=compact,storage=client.request('GET','/'+index+'/_stats/store,docs,segments'),corpus_sha256=checksum)
