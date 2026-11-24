# SPDX-License-Identifier: Apache-2.0
"""Bounded opt-in native routes over an immutable, exclusively owned fixture DB.

Plans come from the canonical Rust generator. No SQL predicate is rewritten.
At most four HTTP responses (each engine helper caps 64 MiB) can be decoded at
once. The pair cap and engine query limits are additional, not RSS guarantees.
"""
import concurrent.futures
import copy
import threading
import time
import urllib.request
import uuid
from comparison import MAX_PAIRS

MAX_WORKERS = 4
MAX_ROWS = 100_000
MAX_VALUE_BYTES = 64 << 20
MAX_SELECTIVITY_PER_MILLE = 100
TABLE = 'wc_staged'


def _queries(queries):
    if (not isinstance(queries, list) or not 1 <= len(queries) <= 8
            or any(not isinstance(q, str) or not q or len(q.encode()) > 200_000 for q in queries)):
        raise ValueError('bounded generated query list required')
    return queries


def _allows(source, rows, size):
    return (0 <= rows <= min(source, MAX_ROWS) and 0 <= size <= MAX_VALUE_BYTES
            and (rows != 0 or size == 0) and rows * 1000 <= source * MAX_SELECTIVITY_PER_MILLE)


def cleanup(engine):
    # Only call for an exclusively owned fixture database, after workers join.
    engine.query("KILL QUERY WHERE startsWith(query_id,'" + engine.prefix + "') AND query_id != currentQueryID() SYNC", logging=False)
    engine.query('DROP TABLE IF EXISTS ' + TABLE + ' SYNC')


def run(engine, parallel, *, source_rows, staged=None):
    queries = _queries(parallel['queries'])
    if not isinstance(source_rows, int) or source_rows < 0:
        raise ValueError('known immutable source row count required')
    if staged is not None:
        if staged.get('table') != TABLE:
            raise ValueError('owned staging table required')
        _queries(staged['queries'])
        if not 0 < len(staged['candidate_select'].encode()) <= 200_000:
            raise ValueError('candidate query bound')
    prefix = engine.prefix + uuid.uuid4().hex[:16]
    ids=[];ids_lock=threading.Lock();stopped=threading.Event()
    start=time.monotonic();deadline=start+60;created=False;refused=False;counts=None

    def query(sql, client=engine, *, cleanup_query=False):
        if not cleanup_query:
            remaining=deadline-time.monotonic()
            if remaining <= 0:
                raise TimeoutError('parallel sweep deadline')
            client=copy.copy(client)
            client.timeout_seconds=min(client.timeout_seconds,max(1,int(remaining)))
        qid=prefix+uuid.uuid4().hex[:16]
        with ids_lock:ids.append(qid)
        return client.query(sql, query_id=qid)

    def cancel():
        stopped.set()
        engine.query("KILL QUERY WHERE startsWith(query_id,'"+prefix+"') AND query_id != currentQueryID() SYNC", logging=False)

    def batches(sqls):
        def action(sql):
            if stopped.is_set():raise RuntimeError('parallel sweep cancelled')
            client=copy.copy(engine)
            client.settings=dict(engine.settings, max_threads=1,
                                 max_memory_usage=int(engine.settings['max_memory_usage'])//MAX_WORKERS)
            client.opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
            return query(sql, client)
        pool=concurrent.futures.ThreadPoolExecutor(max_workers=MAX_WORKERS)
        pending=set();pairs=[];remaining=iter(sqls)
        try:
            for sql in remaining:
                pending.add(pool.submit(action,sql))
                if len(pending)==MAX_WORKERS:break
            while pending:
                done,pending=concurrent.futures.wait(pending,timeout=max(.001,deadline-time.monotonic()),
                                                   return_when=concurrent.futures.FIRST_COMPLETED)
                if not done:raise TimeoutError('parallel sweep deadline')
                for future in done:
                    result=future.result()
                    if len(pairs)+len(result)>MAX_PAIRS:raise ValueError('parallel aggregate pair bound')
                    pairs.extend(result)
                # Admit another request only after consuming completed results.
                # Completed queued replies cannot accumulate behind the pair cap.
                for _ in done:
                    sql=next(remaining,None)
                    if sql is not None:pending.add(pool.submit(action,sql))
            return pairs
        except BaseException:
            cancel()
            raise
        finally:
            stopped.set()
            pool.shutdown(wait=True, cancel_futures=True)

    try:
        if staged is not None:
            # Timed preflight avoids allocating a dense candidate table. The
            # known population belongs to this immutable, verified fixture.
            counts=tuple(map(int,query('SELECT count(),sum(length(v)) FROM ('+staged['candidate_select']+')')[0]))
            if not _allows(source_rows,*counts):
                refused=True
            elif counts[0]==0:
                queries=[]
            else:
                created=True  # Even a failed CREATE can leave owned partial state.
                query('CREATE TABLE '+TABLE+' ENGINE=Memory AS '+staged['candidate_select'])
                actual=tuple(map(int,query('SELECT count(),sum(length(v)) FROM '+TABLE)[0]))
                if actual!=counts:raise RuntimeError('immutable candidate population changed')
                queries=staged['queries']
        pairs=batches(queries) if queries else []
    except BaseException:
        cancel()
        raise
    finally:
        if created:
            query('DROP TABLE IF EXISTS '+TABLE+' SYNC',cleanup_query=True)
    elapsed=(time.monotonic()-start)*1000
    if elapsed>60000:raise TimeoutError('parallel sweep completed after deadline')
    return dict(request_ms=elapsed,pairs=pairs,query_ids=ids,source_queries=len(queries),
                staging_refused=refused,candidate_rows=None if counts is None else counts[0],
                candidate_value_bytes=None if counts is None else counts[1],
                max_workers=MAX_WORKERS,timer='preflight + fresh staging + exact checks + synchronous drop')
