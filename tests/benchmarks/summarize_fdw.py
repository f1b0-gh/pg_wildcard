#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Aggregate paired FDW raw measurements without adding overlapping clocks."""
import argparse,csv,json,statistics
from collections import defaultdict
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('directory');a=p.parse_args();root=Path(a.directory)
groups=defaultdict(list)
for file in sorted(root.glob('*/raw.jsonl')):
    settings=json.loads(file.with_name('settings.json').read_text())
    for line in file.read_text().splitlines():
        row=json.loads(line)
        if 'error' in row:raise SystemExit(f"failed sample in {file}: {row['error']}")
        key=(row['workload'],row['policy'],row['batch_bytes'],row['concurrency'],settings['arguments'].get('timing_off',False),row['route'])
        groups[key].append(row)
fields=['workload','policy','batch_bytes','concurrency','timing_off','route','samples','rows','rows_per_second','http_body_bytes','wall_ms','first_batch_ms','body_wait_ms','arrow_decode_ms','framing_decode_ms','terminal_ms','conversion_ms','consumer_wait_ms','producer_queue_wait_ms','server_ms','read_rows','read_bytes','files_read','parquet_rows_filtered','peak_batch_bytes','peak_rss_mib','max_threads']
fields += [name+'_samples' for name in ('files_read','parquet_rows_filtered','peak_batch_bytes','peak_rss_mib','max_threads')]
rows=[]
for key,samples in sorted(groups.items()):
    result=dict(zip(fields[:6],key));result['samples']=len(samples)
    def med(fn, coverage=None):
        values=[fn(r) for r in samples];values=[v for v in values if v is not None]
        if coverage:result[coverage+'_samples']=len(values)
        return round(statistics.median(values),3) if values else None
    def peak(fn, coverage, divisor=1):
        values=[fn(r) for r in samples];values=[v for v in values if v is not None]
        result[coverage+'_samples']=len(values)
        return (max(values) if divisor==1 else round(max(values)/divisor,3)) if values else None
    result['rows']=med(lambda r:r.get('rows',r.get('scan',{}).get('FDW remote rows')))
    result['http_body_bytes']=med(lambda r:r.get('http_body_bytes',r.get('scan',{}).get('FDW HTTP body bytes')))
    result['wall_ms']=med(lambda r:r['wall_ms'])
    result['rows_per_second']=round(result['rows']*1000/result['wall_ms'],3)
    for field,direct,pg in [('first_batch_ms','first_batch_ms','FDW first batch'),('body_wait_ms','http_body_wait_ms','FDW HTTP body wait'),('arrow_decode_ms','arrow_decode_ms','FDW Arrow decode'),('framing_decode_ms','framing_decode_ms','FDW framing decode'),('terminal_ms','terminal_ms','FDW terminal confirmation'),('conversion_ms','conversion_ms','FDW row conversion'),('consumer_wait_ms','consumer_wait_ms','FDW consumer wait'),('producer_queue_wait_ms','producer_queue_wait_ms','FDW producer queue wait')]:
        result[field]=med(lambda r:r.get(direct,r.get('scan',{}).get(pg)))
    for field,k in [('server_ms','query_duration_ms'),('read_rows','read_rows'),('read_bytes','read_bytes')]:result[field]=med(lambda r:int(r['query_log'][0][k]))
    for field,k in [('files_read','EngineFileLikeReadFiles'),('parquet_rows_filtered','ParquetRowsFilterExpression')]:
        result[field]=med(lambda r:int(r['query_log'][0]['ProfileEvents'][k]) if k in r['query_log'][0]['ProfileEvents'] else None,field)
    result['peak_batch_bytes']=peak(lambda r:r.get('peak_individual_batch_bytes',r.get('scan',{}).get('FDW peak individual batch bytes')),'peak_batch_bytes')
    result['peak_rss_mib']=peak(lambda r:r.get('process_peak',{}).get('rss_kib'),'peak_rss_mib',1024)
    result['max_threads']=peak(lambda r:r.get('process_peak',{}).get('threads'),'max_threads')
    rows.append(result)
with (root/'summary.csv').open('w') as f:
    writer=csv.DictWriter(f,fieldnames=fields,lineterminator="\n");writer.writeheader();writer.writerows(rows)
(root/'summary.json').write_text(json.dumps(rows,indent=2)+'\n')
print(f'{sum(len(v) for v in groups.values())} confirmed samples; {len(rows)} grouped rows')
