"""Inclusive stack attribution from resolved recordings, not exclusive timings."""
import json
from pathlib import Path

root = Path('.local/perf/fdw/flames')
symbolization = json.loads((root / 'symbolization.json').read_text())
rules = {
    'FDW scan callback': lambda f: '::fdw::iterate_foreign_scan' in f,
    'Tuple-building context': lambda f: '::exec_in_context' in f and '::fdw::iterate_foreign_scan' in f,
    'Explicit value/Datum conversion frames': lambda f: any(s in f for s in ('::fdw::values::', '::fdw::datum::')),
    'Local wc_* wrapper': lambda f: any(s in f for s in ('wc_match_wrapper', 'wc_regex_wrapper', 'wc_like_wrapper')),
    'antalya-rs': lambda f: 'antalya_rs[' in f,
    'Arrow IPC': lambda f: 'arrow_ipc[' in f,
    'Base64': lambda f: 'base64[' in f,
}
lines = ['# FDW CPU samples', '',
    'Twenty backend scans per workload, cycles:u at 199 Hz with DWARF unwinding. '
    'A sample counts once in a category when any frame matches its rule. Categories '
    'overlap: tuple building is inside the FDW callback, and Arrow/Base64 can be '
    'inside Antalya. These percentages cannot be added or treated as wall-time stages. '
    'A falling percentage is not by itself proof of an end-to-end speedup.', '',
    '| Binary | Workload | Samples | ' + ' | '.join(rules) + ' |',
    '|---|---|---:|' + '---:|' * len(rules)]
records = []
for profile in symbolization['profiles']:
    directory = root / profile['label'] / profile['workload']
    counts = dict.fromkeys(rules, 0)
    total = 0
    for line in (directory / 'flamegraph.folded').read_text().splitlines():
        stack, weight = line.rsplit(' ', 1)
        weight = int(weight)
        total += weight
        frames = stack.split(';')
        for category, matches in rules.items():
            if any(matches(frame) for frame in frames):
                counts[category] += weight
    if total != profile['samples']:
        raise ValueError('sample count differs from symbolization proof')
    records.append(dict(label=profile['label'], workload=profile['workload'], samples=total,
        inclusive_counts=counts, inclusive_percent={name: count / total * 100 for name, count in counts.items()}))
    lines.append(f"| {profile['label']} | {profile['workload']} | {total} | " +
        ' | '.join(f'{count / total * 100:.1f}%' for count in counts.values()) + ' |')
lines += ['', 'The scan callback and tuple-building context are covered by '
    '[conversion follow-up #17](https://github.com/f1b0-gh/pg_wildcard/issues/17); '
    '[streaming #18](https://github.com/f1b0-gh/pg_wildcard/issues/18) covers the '
    'transport path. Local wc_* wrappers include view handling and matcher/cache '
    'lookup, not just regex execution. Regex-engine changes remain a paper discussion.', '',
    'The explicit conversion-frame category can undercount inlined conversion work. '
    'Use the actual folded stacks and flamegraphs when assessing its boundaries. '
    'The prepared conversion prototype is rejected because the five paired latency '
    'trials did not establish acceptable backend and complete-delivery gains across '
    'the workloads, even where sampled tuple-building cost fell.', '',
    'Original symbolization through /proc/1/root left extension frames unresolved. '
    'The same recordings were reprocessed with stable symbol directories and exact '
    'saved binaries. All eight sample totals are unchanged; five extension frames '
    'remain unresolved across the two broad-recheck profiles, and zero in the other '
    'six. See symbolization.json. No query was replayed to repair the symbols.']
(root / 'cpu-attribution.json').write_text(json.dumps(records, indent=2) + '\n')
(root / 'cpu-attribution.md').write_text('\n'.join(lines) + '\n')
print(root / 'cpu-attribution.md')
