#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Embedded correctness smoke for all comparison distributions and all indicators."""
import argparse
import json
from pathlib import Path
from comparison import corpus_digest, expected_pairs, require_wildcard_target, verify_pairs
from compare_routes import native_run, source_hash, source_sql, sweeps


def main():
    p=argparse.ArgumentParser();p.add_argument('spec',type=Path);a=p.parse_args()
    spec=json.loads(a.spec.read_text())
    require_wildcard_target(spec)
    expected=expected_pairs(spec)
    engine=sweeps.Engine()
    try:
        engine.query('CREATE TABLE templates(id UInt64,v String) ENGINE=MergeTree ORDER BY id')
        for start in range(0,len(spec['templates']),64):
            engine.query('INSERT INTO templates VALUES '+','.join(f'({i},{sweeps.literal(v)})' for i,v in enumerate(spec['templates'][start:start+64],start)))
        engine.query('CREATE TABLE bench(id Int64,v String) ENGINE=MergeTree ORDER BY id')
        engine.query('INSERT INTO bench '+source_sql(spec,0,spec['rows']))
        if source_hash(engine, 'bench') != corpus_digest(spec):
            raise AssertionError('native source values differ from the shared corpus')
        result=native_run(engine,spec['queries'])
        digest=verify_pairs([tuple(p) for p in result['pairs']],expected)
        gated=[]
        if spec.get('wildcard_row_predicates'):
            gate=' OR '.join('('+p+')' for p in spec['wildcard_row_predicates'])
            gated=[q.replace('FROM bench\n','FROM (SELECT id,v FROM bench WHERE '+gate+') AS guarded\n') for q in spec['queries']]
            if any(len(q.encode())>200000 for q in gated):raise AssertionError('wildcard gate smoke budget')
            verify_pairs([tuple(p) for p in native_run(engine,gated)['pairs']],expected)
        engine.query('TRUNCATE TABLE bench');engine.query('INSERT INTO bench SELECT toInt64(id),v FROM templates')
        canary=[(row,int(hit[0]),str(hit[1])) for row,hits in enumerate(spec['template_hits']) for hit in hits]
        verify_pairs([tuple(p) for p in native_run(engine,spec['queries'])['pairs']],canary)
        if gated:verify_pairs([tuple(p) for p in native_run(engine,gated)['pairs']],canary)
        if {p[1] for p in canary}!=set(range(spec['indicators'])):raise AssertionError('canary omitted an indicator')
        print(json.dumps({'variant':spec['variant'],'pairs':len(expected),'sha256':digest,'all_indicator_canary':len(canary),'status':'PASS'}))
    finally:engine.close()

if __name__=='__main__':main()
