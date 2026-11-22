# SPDX-License-Identifier: Apache-2.0
"""Reproduce tables from retained, correlated observations (stdlib only, offline)."""
from collections import defaultdict
import gzip,json,statistics,sys
from pathlib import Path

def require(condition,message):
    if not condition:raise ValueError(message)
def load(path):
    return json.loads(gzip.decompress(path.read_bytes()) if path.suffix=='.gz' else path.read_bytes())
def foreign(plan):
    if plan.get('Node Type')=='Foreign Scan':return plan
    found=[foreign(p) for p in plan.get('Plans',[])];found=[p for p in found if p]
    require(len(found)<=1,'ambiguous scan');return found[0] if found else None

def validate(records,cohort):
    ids=set()
    for r in records:
        require(r.get('status','success')=='success' and r['request_ms']>=0,'failed observation')
        require(r['query_id'] not in ids,'duplicate query ID');ids.add(r['query_id'])
        p=r['profile'];require(int(p['result_rows'])==r['rows'],'server row count mismatch')
        compression=p['settings'].get('output_format_arrow_compression_method')
        if cohort=='streaming-full' and r['route']=='native':pass # historical unmatched codec; never used as uncompressed control
        else:require(compression==r.get('codec','none'),'codec mismatch')
        if 'plan' in r:
            scan=foreign(r['plan']['Plan']);require(scan is not None,'missing scan')
            require(scan['FDW metrics complete'] and not scan['FDW row timing enabled'],'completion/timing mismatch')
            require(scan['Actual Rows']==r['rows']==scan['FDW remote rows'],'backend row count mismatch')
            require(not scan.get('Rows Removed by Filter',0),'local rejection')
            require(scan['FDW peak individual batch bytes']<=16777216,'batch charge limit')
            require(scan['FDW last query ID']==r['query_id'],'PG query correlation')
            if r.get('route')=='pg_backend':require(scan['ClickHouse SQL']==r['selected_sql'],'selected SQL mismatch')
        elif r['route'] in ('direct','direct_antalya_rs'):
            require(r['peak_individual_batch_bytes']<=16777216,'direct charge limit')
            if r['repeat']<0:require(r['value_control'] is True,'warmup values not verified')
        else:require(r['value_control']['rows']==r['rows'],'full values not verified')
        if 'block_size' in r:require(str(p['settings'].get('max_block_size'))==str(r['block_size']),'block setting mismatch')
        if r.get('workload','').startswith('lake_'):
            files=1 if r['workload']=='lake_indexed' else 8
            require(p['profile_events']['EngineFileLikeReadFiles']==files,'file pruning mismatch')
            require(p['profile_events']['ParquetRowsFilterExpression']==files*125000,'Parquet work mismatch')

def table(headers,rows):
    return '\n'.join(['| '+' | '.join(headers)+' |','| '+' | '.join('---' for _ in headers)+' |']+['| '+' | '.join(str(x) for x in row)+' |' for row in rows])
def median(records,key):return statistics.median(r[key] for r in records)
def number(v):return f'{v:.3f}'

def report(root):
    records={};summary={};sections=[]
    for cohort in ('streaming-full','native-uncompressed','streaming-wide-inband','compression-paired','terminal-1000','terminal-100'):
        path=root/cohort/'correlated.json.gz'
        require(path.exists(),'missing cohort: '+cohort)
        rows=load(path);validate(rows,cohort);records[cohort]=rows
        settings=load(root/cohort/'settings.json');groups=defaultdict(list)
        fields=('workload','policy','codec') if cohort=='compression-paired' else ('workload','policy','block_size','route')
        for r in rows:groups[tuple(r[k] for k in fields)].append(r)
        expected_cells={'streaming-full':48,'native-uncompressed':6,'streaming-wide-inband':12,'compression-paired':10,'terminal-1000':48,'terminal-100':48}[cohort]
        require(len(groups)==expected_cells,'missing configuration')
        output=[]
        for key,group in sorted(groups.items()):
            require(sorted(r['repeat'] for r in group)==[-1,0,1,2,3,4],'repetition coverage mismatch')
            measured=[r for r in group if r['repeat']>=0]
            entry=dict(zip(fields,key),n=5,median_ms=median(measured,'request_ms'),min_ms=min(r['request_ms'] for r in measured),max_ms=max(r['request_ms'] for r in measured))
            pkey='server_ms' if cohort=='compression-paired' else 'query_duration_ms'
            entry['server_ms']=statistics.median(r['profile'][pkey] for r in measured)
            events_key='events' if cohort=='compression-paired' else 'profile_events'
            for event in ('NetworkSendElapsedMicroseconds','DiskReadElapsedMicroseconds','OSCPUWaitMicroseconds','UserTimeMicroseconds','SystemTimeMicroseconds'):
                entry[event]=statistics.median(r['profile'][events_key].get(event,0) for r in measured)

            if measured[0]['route'] in ('direct','direct_antalya_rs'):
                for field in ('http_body_wait_ms','arrow_decode_ms','framing_decode_ms','terminal_ms','flush_ms','observe_ms','polls','flush_attempts','poll_wait_ms','http_body_bytes'):
                    entry[field]=median(measured,field)
                entry['polls_range']=[min(r['polls'] for r in measured),max(r['polls'] for r in measured)]
            output.append(entry)
        summary[cohort]=output
        rows=[[*(e[f] for f in fields),number(e['median_ms']),number(e['min_ms']),number(e['max_ms']),number(e['server_ms'])] for e in output]
        sections.append('## '+cohort+'\n\n'+table([*fields,'median ms','min ms','max ms','server median ms'],rows))
        event_names=('NetworkSendElapsedMicroseconds','DiskReadElapsedMicroseconds','OSCPUWaitMicroseconds','UserTimeMicroseconds','SystemTimeMicroseconds')
        sections.append('Native ProfileEvents medians (ms, aggregate across query threads where applicable; overlapping diagnostics). Missing event counters mean zero.\n\n'+table([*fields,'network send/wait','read syscall','CPU scheduling wait','user CPU','system CPU'],[[*(e[f] for f in fields),*(number(e[k]/1000) for k in event_names)] for e in output]))
        direct=[e for e in output if 'terminal_ms' in e]
        if direct:
            fields2=['body wait','Arrow decode','framing decode','terminal','flush','observe','poll wait']
            keys=['http_body_wait_ms','arrow_decode_ms','framing_decode_ms','terminal_ms','flush_ms','observe_ms','poll_wait_ms']
            sections.append('Overlapping direct-client diagnostics (ms); never add these clocks. Flush/observe are aggregate per operation.\n\n'+table([*fields,'body bytes',*fields2,'poll count range'],[[*(e[f] for f in fields),int(e['http_body_bytes']),*(number(e[k]) for k in keys),e['polls_range']] for e in direct]))
    paired=[];phase_groups=defaultdict(list);ttl_seen=False
    for path in sorted((root/'cache-paired').glob('*-*/correlated.json.gz')):
        label=path.parent.name;rows=load(path);validate(rows,'cache');settings=load(path.parent/'settings.json')
        controls=load(path.parent/'value-controls.json');require(len(controls)==48,'missing manual value controls')
        for r in controls:require(r['value_control']['rows']==(0 if 'never' in r['pattern'] else 125),'manual row contract')
        if label=='candidate-ttl':
            ttl_seen=True
            require(len(rows)==15,'TTL cohort incomplete')
            for n in range(5):
                cohort={r['phase']:r for r in rows if r['round']==n}
                require(cohort['ttl_insert']['cache_hits']==0 and cohort['ttl_insert']['probes']==2,'cold TTL')
                require(cohort['ttl_middle']['cache_hits']==1 and cohort['ttl_middle']['probes']==0,'middle TTL')
                require(30<=cohort['ttl_middle']['age_since_insert_completion_seconds']<59,'middle age')
                require(cohort['ttl_expired']['cache_hits']==0 and cohort['ttl_expired']['probes']==2,'TTL expiry')
                require(cohort['ttl_expired']['age_since_insert_completion_seconds']>=61,'expiry age')
            continue
        variant=label.split('-',1)[1];hot=[r for r in rows if r['phase']=='capacity_hot']
        require(len(rows)==98 and len(hot)==40,'cache cohort incomplete')
        cap=load(path.parent/'capacity-summary.json')[0];require(cap['round_seconds']<60,'TTL confounds capacity')
        hits=sum(r['cache_hits'] for r in hot);probes=sum(r['probes'] for r in hot)
        require(hits==(32 if variant=='baseline' else 40) and probes==(16 if variant=='baseline' else 0),'capacity result differs')
        paired.append(dict(cohort=label,variant=variant,hot_hit_rate=hits/40,hot_probes=probes,hot_median_ms=median(hot,'request_ms'),hot_total_ms=sum(r['request_ms'] for r in hot)))
        for r in rows:phase_groups[variant,r['phase']].append(r)
        for phase,hit,probe in [('reuse_cold',0,2),('reuse_warm1',1,0),('reuse_warm2',1,0),('churn',0,2),('settings_0',0,2),('settings_1',1,0),('settings_2',0,2),('settings_3',1,0)]:
            matches=[r for r in rows if r['phase']==phase];require(len(matches)==(3 if phase=='churn' else 1),'cache phase incomplete')
            require(all(r['cache_hits']==hit and r['probes']==probe for r in matches),'reuse/settings boundary')
    require(ttl_seen and len(paired)==10,'cache/TTL cohort missing')
    if paired:
        require(len(paired)==10,'paired cache rounds missing');summary['cache-capacity']=paired
        sections.append('## Adaptive cache: five alternating baseline/candidate rounds\n\n'+table(['cohort','hot hit rate','hot probes','hot median ms','40 hot queries total ms'],[[e['cohort'],e['hot_hit_rate'],e['hot_probes'],number(e['hot_median_ms']),number(e['hot_total_ms'])] for e in paired]))
        phases=[]
        for (variant,phase),rs in sorted(phase_groups.items()):
            if phase.startswith('capacity'):continue
            e=dict(variant=variant,phase=phase,n=len(rs),median_ms=median(rs,'request_ms'),preparation_ms=statistics.median(foreign(r['plan']['Plan'])['FDW plan preparation'] for r in rs),median_connection_setup_ms=median(rs,'connection_setup_ms'))
            phases.append(e)
        summary['cache-phases']=phases
        sections.append('Setup is measured separately; persistent-backend request clocks exclude it.\n\n'+table(['variant','phase','n','request median ms','preparation median ms','connection setup median ms'],[[e['variant'],e['phase'],e['n'],number(e['median_ms']),number(e['preparation_ms']),number(e['median_connection_setup_ms'])] for e in phases]))
    (root/'report.json').write_text(json.dumps(summary,indent=2)+'\n')
    (root/'tables.md').write_text('<!-- SPDX-License-Identifier: Apache-2.0 -->\n# Reproduced FDW tuning tables\n\nFive measured repetitions plus excluded value-check warmups per streaming/terminal cell. Cache cold/hot phases are intentional and separately labelled. Shared-host observations, not isolated production latency. Native HTTP never has a terminal-policy contract; its policy label only marks scheduling alongside that client configuration. The original streaming-full native codec was unmatched: use native-uncompressed as a separate control, not a paired client-overhead estimate.\n\n'+'\n\n'.join(sections)+'\n')

if __name__=='__main__':report(Path(sys.argv[1]))
