# SPDX-License-Identifier: Apache-2.0
"""Exact multisets through real Memory staging, including duplicates and byte data."""
from collections import Counter


def check(cases, q, rows, fail):
    for case in cases:
        column = case['column']
        q(f'CREATE TABLE staged_source(id UInt32, {column} String, indicator_id UInt32, view UInt32) ENGINE=Memory')
        values = [f"({i},unhex('{h}'),{i},{i})" for i, h in enumerate(case['values_hex'])]
        for offset in range(0, len(values), 64):
            q('INSERT INTO staged_source VALUES ' + ','.join(values[offset:offset + 64] * 2))
        wanted = Counter(tuple(v) for v in case['expected'])
        wanted.update(tuple(v) for v in case['expected'])
        try:
            q('CREATE TABLE staged_candidates ENGINE=Memory AS ' + case['candidate_select'])
            for prefer_columns in (0, 1):
                actual = Counter()
                for sql in case['queries']:
                    actual.update(tuple([int(x) for x in v[:-1]] + [v[-1]]) for v in rows(
                        sql + f' SETTINGS prefer_column_name_to_alias={prefer_columns}, max_threads=1'))
                if actual != wanted:
                    fail(f"staged {case['name']}: extra={list((actual-wanted).items())[:3]}, "
                         f"missing={list((wanted-actual).items())[:3]}")
            print(f"staged {case['name']}: exact multisets, duplicate rows/rules, byte and long values", flush=True)
        finally:
            q('DROP TABLE IF EXISTS staged_candidates SYNC')
            q('DROP TABLE staged_source SYNC')
