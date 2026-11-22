#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Create deterministic million-row Parquet and genuine Puffin/PG fixtures."""
import argparse
from pathlib import Path
import subprocess
import pyarrow as pa
import pyarrow.parquet as pq

p=argparse.ArgumentParser()
p.add_argument('--directory',required=True)
p.add_argument('--remote-prefix',default='/var/lib/clickhouse/user_files/benchmark/fdw_profile')
p.add_argument('--cli',required=True)
p.add_argument('--index-directory',required=True)
a=p.parse_args()
out=Path(a.directory).resolve();out.mkdir(parents=True,exist_ok=True)
index=Path(a.index_directory).resolve();index.mkdir(parents=True,exist_ok=True)
for f in range(8):
    ids=list(range(f*125000,(f+1)*125000))
    pq.write_table(pa.table({'id':pa.array(ids,type=pa.int64()),'url':['evil.test/powershell' if f==0 and i%1000==0 else 'benign.test/normal' for i in ids],'extra':['x'*16]*len(ids)}),out/f'f{f}.parquet',compression='snappy')
subprocess.run([a.cli,'index','--column','url','--views','raw','--snapshot-id','1','--out',str(index/'lake.puffin'),*[str(out/f'f{f}.parquet')+'='+a.remote_prefix+f'/f{f}.parquet' for f in range(8)]],check=True)
subprocess.run([a.cli,'export-pg',str(index/'lake.puffin'),'--name','profile_lake','--out-dir',str(index/'export')],check=True)
