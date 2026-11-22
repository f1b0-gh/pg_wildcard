# SPDX-License-Identifier: Apache-2.0
"""Untimed exact-value checks for the bounded deterministic FDW fixtures."""
import struct
from comparison import MAX_BYTES, OracleMismatch

WORKLOADS=('narrow','smalltext','wide1k','broad_recheck','selective')
HEADER=b'PGCOPY\n\xff\r\n\0'+b'\0'*8


def expected_row(workload, row):
    if workload not in WORKLOADS:raise ValueError('unknown FDW fixture')
    identifier=struct.pack('!q',row)
    if workload=='narrow':return (identifier,)
    url=b'evil.test/powershell' if workload=='selective' else (
        b'benign.test/normal' if workload=='wide1k' or row%10000 else b'evil.test/powershell')
    return (identifier,url,b'x'*(1024 if workload=='wide1k' else 16))


def verify_copy(raw, workload, rows):
    """Verify every field and the complete unique row-ID set, in any order.

    The seen bitmap is at most 1 MiB; payloads remain bounded at 64 MiB. This
    validation deliberately runs after the measured complete-consumption clock.
    """
    if workload not in WORKLOADS or not 0<rows<=1000000:raise ValueError('invalid FDW fixture bounds')
    if workload=='selective' and rows>1000:raise ValueError('invalid selective fixture bounds')
    if len(raw)>MAX_BYTES or raw[:19]!=HEADER:raise OracleMismatch('COPY header/byte bound differs')
    seen=bytearray(rows);offset=19;count=0;fields=1 if workload=='narrow' else 3
    def take(n):
        nonlocal offset
        if n<0 or offset+n>len(raw):raise OracleMismatch('truncated COPY field')
        value=raw[offset:offset+n];offset+=n;return value
    while True:
        n=struct.unpack('!h',take(2))[0]
        if n==-1:break
        if n!=fields or count>=rows:raise OracleMismatch('COPY shape/row bound differs')
        values=tuple(take(struct.unpack('!i',take(4))[0]) for _ in range(n))
        if len(values[0])!=8:raise OracleMismatch('COPY row ID width differs')
        row=struct.unpack('!q',values[0])[0]
        stride=1000 if workload=='selective' else 1
        ordinal=row//stride
        if row%stride or not 0<=ordinal<rows or seen[ordinal]:raise OracleMismatch('COPY duplicate/out-of-range row')
        if values!=expected_row(workload,row):raise OracleMismatch('COPY field value differs')
        seen[ordinal]=1;count+=1
    if offset!=len(raw) or count!=rows:raise OracleMismatch('COPY trailing data or missing rows')
    return {'rows':count,'copy_bytes':len(raw),'oracle':'every field and complete unique row-ID set'}
