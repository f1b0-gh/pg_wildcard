#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Shared bounded fixtures, exact multiset oracle and measurement schedule."""
import hashlib
import struct

MAX_PAIRS = 100000
MAX_BYTES = 64 << 20
class OracleMismatch(ValueError):
    pass


HEADER = b'PGCOPY\n\xff\r\n\0'


def require_wildcard_target(spec):
    """Admit the synthetic benchmark target, not an inferred application field.

    Callers of the library still own schema resolution and observable policy.
    These fixtures intentionally cover only the explicitly selected families.
    """
    target = spec.get('search_target')
    if isinstance(target, dict):
        if target.get('kind') == 'field' and target.get('data_type') == 'wildcard':
            return
        if (target.get('kind') == 'observable'
                and target.get('observable_type') in ('domain', 'url', 'path')
                and target.get('wildcard_enabled') is True
                and spec.get('query_contract') == 'case_sensitive_wildcard'):
            return
    raise ValueError('comparison requires a declared wildcard field or explicitly enabled wildcard observable')


def pair_digest(pairs):
    digest = hashlib.sha256()
    for row, indicator, view in sorted(pairs):
        digest.update(f'{row}\t{indicator}\t{view}\n'.encode())
    return digest.hexdigest()


def verify_pairs(actual, expected):
    if len(actual) > MAX_PAIRS or sorted(actual) != sorted(expected):
        raise OracleMismatch('complete row/indicator/view multiset differs')
    return pair_digest(actual)


def copy_pairs(raw):
    """Decode all binary COPY fields after timing; reject malformed/trailing data."""
    if len(raw) > MAX_BYTES or len(raw) < 21 or raw[:11] != HEADER:
        raise OracleMismatch('invalid or oversized binary COPY')
    flags, extension = struct.unpack_from('!ii', raw, 11)
    if flags != 0 or extension < 0 or extension > len(raw) - 21:
        raise OracleMismatch('unsupported COPY header')
    offset = 19 + extension
    pairs = []
    def take(length):
        nonlocal offset
        if length < 0 or offset + length > len(raw):
            raise OracleMismatch('truncated COPY value')
        value = raw[offset:offset + length]
        offset += length
        return value
    while True:
        fields = struct.unpack('!h', take(2))[0]
        if fields == -1:
            if offset != len(raw): raise OracleMismatch('trailing COPY data')
            return pairs
        if fields != 3 or len(pairs) >= MAX_PAIRS:
            raise OracleMismatch('COPY shape or pair bound exceeded')
        values = [take(struct.unpack('!i', take(4))[0]) for _ in range(3)]
        if len(values[0]) != 8 or len(values[1]) != 8:
            raise OracleMismatch('COPY integer width differs')
        try: view = values[2].decode('ascii')
        except UnicodeDecodeError: raise OracleMismatch('invalid view') from None
        if view not in ('raw', 'lower', 'decoded', 'revdomain', 'winpath'):
            raise OracleMismatch('unknown view')
        pairs.append((struct.unpack('!q', values[0])[0], struct.unpack('!q', values[1])[0], view))


def corpus_value(spec, row):
    if row % spec['stride'] == 0:
        return spec['templates'][(row // spec['stride']) % len(spec['templates'])]
    return spec['benign_prefix'] + str(row) + spec.get('benign_suffix', '')


def expected_pairs(spec, max_pairs=MAX_PAIRS):
    rows, stride = spec['rows'], spec['stride']
    if not 0 < rows <= 1000000 or not 0 < stride <= 1000000:
        raise ValueError('invalid corpus bounds')
    templates = spec['template_hits']
    rare = (rows + stride - 1) // stride
    count = sum(((rare + len(templates) - 1 - i) // len(templates)) * len(hits)
                for i, hits in enumerate(templates))
    if count > max_pairs: raise ValueError('oracle exceeds pair budget')
    return [(row, indicator, view) for row in range(0, rows, stride)
            for indicator, view in templates[(row // stride) % len(templates)]]


def corpus_digest(spec):
    digest = hashlib.sha256()
    for row in range(spec['rows']):
        digest.update(f'{row}\t{corpus_value(spec, row)}\n'.encode())
    return digest.hexdigest()


def schedule(variants, repeats):
    if not variants or len(set(variants)) != len(variants) or not 1 <= repeats <= 20:
        raise ValueError('invalid schedule')
    result = [(True, 0, v) for v in variants]
    for repeat in range(repeats):
        start = repeat % len(variants)
        order = variants[start:] + variants[:start]
        result.extend((False, repeat, v) for v in order)
    return result


def elastic_pairs(responses, patterns):
    if len(responses) != len(patterns): raise OracleMismatch('missing Elastic response')
    pairs=[]
    for response, pattern in zip(responses, patterns):
        if 'error' in response or response.get('timed_out', True) or response.get('_shards', {}).get('failed', 1):
            raise OracleMismatch('Elastic partial/failed response')
        hits=response['hits']; total=hits['total']
        if total['relation']!='eq' or total['value']!=len(hits['hits']):
            raise OracleMismatch('Elastic truncated/inexact response')
        pairs.extend((int(hit['_source']['id']), pattern['id'], pattern['view']) for hit in hits['hits'])
        if len(pairs)>MAX_PAIRS: raise OracleMismatch('Elastic pair bound')
    return pairs


def overlap_peak(intervals):
    events=sorted([(start,1) for start,_ in intervals]+[(end,-1) for _,end in intervals])
    active=peak=0
    for _,delta in events:
        active+=delta;peak=max(peak,active)
    return peak
