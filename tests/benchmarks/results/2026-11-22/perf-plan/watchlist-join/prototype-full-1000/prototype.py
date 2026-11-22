"""Throwaway raw-view rule-index join; no public API or performance claim."""
import re

MAX_VALUE_BYTES = 256
FALLBACK = '_wc_no_anchor'
OVERFLOW = '_wc_long_value'


def ident(value):
    if not re.fullmatch(r'[A-Za-z_][A-Za-z_0-9]*', value):
        raise ValueError('prototype requires a simple SQL identifier')
    return '`' + value + '`'


def render(spec, *, name='percolator_mv', source='ingest_src',
           hits='ingest_hits', watch='watch'):
    if any(p['view'] != 'raw' for p in spec['patterns']):
        return None, {'fallback': 'prototype_raw_view_only'}
    name, source, hits, watch = map(ident, (name, source, hits, watch))
    # Both the anchor and final checks derive from the same FINAL rule read.
    bound_needles = (
        'if(needles_regex = regex AND needles_view = view, '
        "arraySlice(needles, 1, 3), CAST([], 'Array(String)'))"
    )
    gram = 'arrayElement(bound_needles, 1)'
    printable = (
        f'length({gram}) = 3 AND '
        f'arrayAll(i -> ascii(substring({gram}, i, 1)) BETWEEN 32 AND 126, [1,2,3])'
    )
    anchor = f"if({printable}, {gram}, '{FALLBACK}')"
    # Cap extraction itself, independently of lazy evaluation of the overflow IF.
    grams = (
        'arrayDistinct(arrayMap(i -> substring(v, i + 1, 3), '
        f'range(toUInt64(greatest(toInt64(least(length(v), {MAX_VALUE_BYTES})) - 2, 0)))))'
    )
    keys = (
        f"if(length(v) > {MAX_VALUE_BYTES}, ['{OVERFLOW}'], "
        f"arrayConcat({grams}, ['{FALLBACK}']))"
    )
    sql = f'''CREATE MATERIALIZED VIEW {name} TO {hits} AS
SELECT src.id AS id, rule.id AS indicator_id, 'raw' AS view
FROM (
    SELECT id, v, arrayJoin({keys}) AS gram_key
    FROM {source}
) AS src
ALL INNER JOIN (
    SELECT id, regex, bound_needles,
           arrayJoin([{anchor}, '{OVERFLOW}']) AS gram_key
    FROM (
        SELECT id, regex, {bound_needles} AS bound_needles
        FROM {watch} FINAL WHERE active = 1 AND view = 'raw'
    )
) AS rule ON src.gram_key = rule.gram_key
WHERE arrayAll(n -> position(src.v, n) > 0, rule.bound_needles)
  AND match(src.v, rule.regex)'''
    return sql, {'strategy': 'raw_rule_anchor_join',
                 'max_value_bytes_before_fallback': MAX_VALUE_BYTES,
                 'maximum_source_keys': MAX_VALUE_BYTES - 1,
                 'index_side': 'current active watchlist, not document storage',
                 'status': 'unqualified local prototype'}
