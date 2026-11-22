# SPDX-License-Identifier: Apache-2.0
"""Production byte-dispatch SQL: full multisets across index lifecycle states."""
from collections import Counter


def check(spec, q, rows, fail):
    for case in spec['cases']:
        q('CREATE TABLE bd (id UInt32, v String, view String, indicator_id String, '
          '_wc_dispatch0_value String, _wc_dispatch0_tokens Array(String)) ENGINE=MergeTree ORDER BY id')
        values = [f"({i}," + ','.join([f"unhex('{h}')"] * 4) + ", ['wrong'])"
                  for i, h in enumerate(case.get('values_hex', spec['values_hex']))]
        for offset in range(0, len(values), 64):
            # Preserve duplicate source rows as well as duplicate rule IDs.
            q('INSERT INTO bd VALUES ' + ','.join(values[offset:offset + 64] * 2))
        expected = Counter(tuple(row) for row in case['expected'])
        expected.update(tuple(row) for row in case['expected'])
        for phase in ('absent', 'unmaterialized', 'materialized', 'dropped'):
            print(f"byte dispatch {case['name']}: {phase}", flush=True)
            if phase == 'unmaterialized':
                q(case['ddl'])
            elif phase == 'materialized':
                q('ALTER TABLE bd MATERIALIZE INDEX byte_grams SETTINGS mutations_sync=2')
            elif phase == 'dropped':
                q('ALTER TABLE bd DROP INDEX byte_grams')
            for skipping in (0, 1):
                for prefer_columns in (0, 1):
                    actual = Counter()
                    for sql in case['queries']:
                        settings = (f' SETTINGS use_skip_indexes={skipping}, '
                                    f'prefer_column_name_to_alias={prefer_columns}')
                        try:
                            actual.update((int(r[0]), int(r[1]), r[2]) for r in rows(sql + settings))
                        except Exception as exc:
                            # Native HTTPError exposes the engine explanation in
                            # its response body; these are synthetic test inputs.
                            detail = exc.read(4096).decode(errors='replace') if hasattr(exc, 'read') else str(exc)
                            raise RuntimeError(f"byte dispatch {case['name']} {phase} "
                                               f"{settings}: {detail}") from exc
                    if 'candidate_rows' in case:
                        # This renderer's explicit gated source precedes ARRAY JOIN.
                        source = case['queries'][0].split('\nFROM (', 1)[1].rsplit('\n)\nARRAY JOIN ', 1)[0]
                        candidates = int(rows('SELECT count() FROM (' + source + ')' + settings)[0][0])
                        if candidates != case['candidate_rows'] * 2:
                            fail(f"byte dispatch {case['name']}: candidate rows {candidates}, "
                                 f"expected {case['candidate_rows'] * 2}")
                    if actual != expected:
                        fail(f"byte dispatch {case['name']} {phase} skip={skipping} "
                             f"prefer_columns={prefer_columns}: "
                             f"extra={list((actual-expected).items())[:3]}, "
                             f"missing={list((expected-actual).items())[:3]}")
            if phase == 'materialized' and case['index_gate']:
                for sql in case['queries']:
                    rows(sql + " SETTINGS force_data_skipping_indices='byte_grams'")
        q('DROP TABLE bd')
        print(f"byte dispatch {case['name']}: index lifecycle, aliases and exact multisets")
