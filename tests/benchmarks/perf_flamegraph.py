#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Render actual `perf script` samples as a static SVG flame graph (stdlib)."""
import argparse
from collections import Counter
import hashlib
from html import escape
from pathlib import Path
import re

p=argparse.ArgumentParser();p.add_argument('input');p.add_argument('output');a=p.parse_args()
counts=Counter()
for block in Path(a.input).read_text().split('\n\n'):
    frames=[]
    for line in block.splitlines():
        m=re.match(r'\s+[0-9a-f]+\s+(.+)\s+\((.+)\)\s*$',line)
        if m:frames.append(m.group(1).split('+0x')[0].strip()+' ['+Path(m.group(2)).name+']')
    if frames:counts[tuple(reversed(frames))]+=1
if not counts:raise SystemExit('No sampled stacks: refusing to invent a flame graph')
total=sum(counts.values());tree={}
for frames,n in counts.items():
    for depth in range(1,len(frames)+1):tree[frames[:depth]]=tree.get(frames[:depth],0)+n
width=1600;step=20;height=(max(map(len,counts))+2)*step
svg=[f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">', '<style>text{font:11px monospace}rect{stroke:white;stroke-width:.4}</style>',f'<text x="5" y="14">PostgreSQL FDW CPU samples: {total}; width = sampled cycles:u events; inclusive stacks</text>']
def draw(prefix,x):
    children=sorted(k for k in tree if len(k)==len(prefix)+1 and k[:-1]==prefix)
    for k in children:
        w=tree[k]/total*width;y=height-(len(k)+1)*step
        color=hashlib.sha256(k[-1].encode()).digest();fill=f'rgb(245,{100+color[0]%110},{40+color[1]%80})'
        svg.append(f'<g><title>{escape(k[-1])}: {tree[k]} samples ({tree[k]/total*100:.2f}%)</title><rect x="{x:.2f}" y="{y}" width="{w:.2f}" height="19" fill="{fill}"/>')
        if w>30:svg.append(f'<text x="{x+3:.2f}" y="{y+13}">{escape(k[-1][:max(0,int(w/7)-1)])}</text>')
        svg.append('</g>');draw(k,x);x+=w

draw((),0);svg.append('</svg>');Path(a.output).write_text('\n'.join(svg))
Path(a.output).with_suffix('.folded').write_text(''.join(';'.join(k)+' '+str(n)+'\n' for k,n in sorted(counts.items())))
