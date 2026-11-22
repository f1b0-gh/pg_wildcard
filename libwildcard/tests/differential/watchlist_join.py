# SPDX-License-Identifier: Apache-2.0
"""Execute production watchlist DDL; compare full multisets, never just sets."""
from collections import Counter
import json


def check(spec, q, rows, fail):
    q("CREATE TABLE jv (id UInt32, v String) ENGINE=Memory")
    q("INSERT INTO jv VALUES " + ", ".join(
        f"({i}, unhex('{h}'))" for i, h in enumerate(spec['values_hex'])))
    q(spec['table_ddl'])
    q("CREATE TABLE js (id UInt32, v String, nested Tuple(value UInt32), "
      "_wc0_value String, _wc1_key String, gram_key String, regex String, i UInt32, "
      "v_alt String MATERIALIZED concat(v, '!')) ENGINE=Memory")
    for case in spec['cases']:
        q(case['hits_ddl'])
        q(case['mv'])
        for phase in ('bound', 'stale_regex', 'stale_view', 'empty_metadata',
                      'updated', 'deactivated', 'empty_watchlist'):
            q('TRUNCATE TABLE jw')
            for insert in spec['inserts']:
                q(insert)
            if phase in ('stale_regex', 'stale_view', 'empty_metadata'):
                q('CREATE TABLE jw_saved ENGINE=Memory AS SELECT * FROM jw FINAL')
                q('TRUNCATE TABLE jw')
                projection = {
                    'stale_regex': "id, view, '.*', needles, needles_regex, needles_view",
                    'stale_view': "id, 'raw', regex, needles, needles_regex, 'lower'",
                    'empty_metadata': 'id, view, regex',
                }[phase]
                columns = ('id, view, regex' if phase == 'empty_metadata' else
                           'id, view, regex, needles, needles_regex, needles_view')
                q(f'INSERT INTO jw ({columns}) SELECT {projection} FROM jw_saved')
                q('DROP TABLE jw_saved')
            elif phase == 'updated':
                # New version supersedes a bound rule without refreshing metadata.
                q("INSERT INTO jw SELECT id, view, '.*', needles, needles_regex, "
                  "needles_view, active, updated_at + INTERVAL 1 SECOND FROM jw FINAL WHERE id = 901")
                q("INSERT INTO jw (id, view, regex) VALUES (999, 'raw', 'abc')")
            elif phase == 'deactivated':
                q("INSERT INTO jw SELECT id, view, regex, needles, needles_regex, "
                  "needles_view, 0, updated_at + INTERVAL 1 SECOND FROM jw FINAL")
            elif phase == 'empty_watchlist':
                q('TRUNCATE TABLE jw')
            q('TRUNCATE TABLE js')
            q('TRUNCATE TABLE jh')
            # Identical rows must survive as two occurrences, even for repeated
            # trigrams and multiple rules with identical regexes.
            source = "SELECT id, v, tuple(id + 10), 'kept0', 'kept1', 'kept2', 'kept3', id + 20 FROM jv"
            q('INSERT INTO js (id, v, nested, _wc0_value, _wc1_key, gram_key, regex, i) ' +
              source + ' UNION ALL ' + source)
            actual_result = json.loads(q('SELECT * FROM jh').data())
            exact_result = json.loads(q(case['oracle']).data())
            actual = Counter(tuple(row) for row in actual_result['data'])
            exact = Counter(tuple(row) for row in exact_result['data'])
            if actual != exact:
                fail(f"join {case['name']} {phase}: multiset differs, "
                     f"extra={list((actual-exact).items())[:3]}, missing={list((exact-actual).items())[:3]}")
            if actual_result['meta'] != exact_result['meta']:
                fail(f"join {case['name']} {phase}: projected names/types differ")
            if case['name'] == 'plain' and phase == 'bound':
                rust = Counter(tuple(row) for row in spec['expected'])
                rust.update(tuple(row) for row in spec['expected'])
                if actual != rust:
                    fail('join plain bound: Rust pair multiset differs')
        q('DROP VIEW join_mv')
        q('DROP TABLE jh')
        print(f"watchlist join {case['name']}: seven lifecycle phases, exact multisets and projections")
    for table in ('js', 'jw', 'jv'):
        q(f'DROP TABLE {table}')
