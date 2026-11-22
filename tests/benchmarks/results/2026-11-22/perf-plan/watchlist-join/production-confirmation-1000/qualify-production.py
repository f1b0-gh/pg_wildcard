import argparse,json,sys
from pathlib import Path
sys.path.insert(0,str(Path('libwildcard/tests/differential').resolve()))
from watchlist_join import check
p=argparse.ArgumentParser();p.add_argument('spec');p.add_argument('--container');a=p.parse_args()
if a.container:
 from native_backend import NativeSession
 s=NativeSession(a.container)
else:
 from chdb import session
 s=session.Session()
def q(sql):return s.query(sql,'JSONCompact')
def rows(sql):
 r=q(sql).data();return json.loads(r)['data'] if r.strip() else []
failures=[]
def fail(message):failures.append(message);print('FAIL',message,flush=True)
try:
 check(json.loads(Path(a.spec).read_text())['watchlist_join'],q,rows,fail)
except Exception as e:
 if hasattr(e,'read'):print(e.read(8192).decode(errors='replace'),flush=True)
 raise
print('RESULT:','PASS' if not failures else f'FAIL ({len(failures)})',flush=True)
sys.exit(bool(failures))
