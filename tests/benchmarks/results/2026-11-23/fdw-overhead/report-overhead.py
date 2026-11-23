#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Verify and summarize the five alternating FDW overhead cohorts (stdlib only).

No database requests, builds, uploads, or source-tree mutations. Inputs are owned
local benchmark artifacts, not untrusted downloaded archives. Fail closed on
missing or inconsistent evidence; --template works before measurements exist.
"""
import argparse
from collections import Counter
import hashlib
import json
import math
from pathlib import Path
import re
import statistics
import sys

LABELS = ('baseline', 'optimized')
WORKLOADS = ('narrow', 'smalltext', 'wide1k', 'broad_recheck')
ROUTES = ('direct', 'pg_backend', 'pg_copy')
ORACLE = 'every field and complete unique row-ID set'
ENGINE = re.compile(r'(?:regex_automata|regex|aho_corasick)(?:\[[^\]]+\])?::')
MATCHER_CALL = re.compile(r'::Matcher>?::is_match\b')
FRAME = re.compile(r'\s+[0-9a-f]+\s+(.+)\s+\((.+)\)\s*$')
DEFINITION = (
    'Non-matcher sampled share = weight of stacks with neither a visible '
    'Matcher::is_match frame nor a regex, regex_automata, or aho_corasick frame '
    '/ total folded stack weight. The intended boundary excludes the whole '
    'Matcher::is_match call, including its view/scratch work; engine frames '
    'also identify matcher samples when that caller is inlined or missing. '
    'The JSON reports complements of visible Matcher::is_match alone and '
    'engine frames alone separately, plus their union used for the proxy. '
    'This complement includes PostgreSQL, FDW, transport, conversion, wrapper, '
    'view/cache work, unresolved frames, and matcher work hidden by inlining or '
    'incomplete unwinding. It is not an exclusive stage measurement. '
    'Both binaries use perf --no-inline and exact saved ELF symbols: recorded '
    'DWARF-unwound frames have no synthetic inline-frame expansion. Therefore '
    'even fully resolved ELF symbols do not reveal all inlined Matcher::is_match '
    'work; the complements cannot exactly isolate everything outside that call. '
    'Estimated non-matcher user CPU/row = process user CPU/row times that share. '
    'cycles:u sampling and process CPU ticks measure different quantities; '
    'this multiplication is an attribution proxy, not exact CPU accounting. '
    'System CPU is reported separately and is not apportioned by user-only samples.'
)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path):
    return json.loads(path.read_text())


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1048576), b''):
            h.update(chunk)
    return h.hexdigest()


def number(value, positive=False):
    require(isinstance(value, (int, float)) and not isinstance(value, bool)
            and math.isfinite(value) and (value > 0 if positive else value >= 0),
            f'invalid numeric measurement: {value!r}')
    return value


def stats(values):
    require(bool(values), 'empty observations')
    return dict(n=len(values), median=statistics.median(values), min=min(values), max=max(values))


def paired_change(before, after):
    require(len(before) == len(after) and bool(before), 'unpaired observations')
    ratios = [number(a) / number(b, positive=True) for b, a in zip(before, after)]
    return dict(paired_optimized_over_baseline=stats(ratios),
                paired_reduction_percent=stats([(1 - r) * 100 for r in ratios]),
                every_pair_at_least_half=all(r <= .5 for r in ratios))


def rows_for(workload):
    require(workload in WORKLOADS, 'unknown workload')
    return 32768 if workload == 'wide1k' else 1000000


def check_copy(control, workload):
    rows = rows_for(workload)
    # Header 19 + trailer 2; per row 2 + field length prefixes + exact payload.
    # Three-field rows include 18 URL bytes (benign.test/normal), not 17.
    expected_bytes = 21 + rows * (14 if workload == 'narrow' else
                                  1064 if workload == 'wide1k' else 56)
    if workload in ('smalltext', 'broad_recheck'):
        expected_bytes += 2 * (rows // 10000)
    require(control.get('rows') == rows and control.get('oracle') == ORACLE
            and control.get('copy_bytes') == expected_bytes,
            f'{workload}: missing or inconsistent exact COPY control')


def folded_counts(path):
    counts = Counter()
    for line in path.read_text().splitlines():
        stack, weight = line.rsplit(' ', 1)
        require(stack and int(weight) > 0, 'invalid folded stack')
        counts[stack] += int(weight)
    require(bool(counts), 'no folded samples')
    return counts


def folded_attribution(path):
    counts = folded_counts(path)
    rules = {
        'fdw_scan': lambda s: '::fdw::iterate_foreign_scan' in s,
        'conversion': lambda s: '::fdw::values::' in s or '::fdw::datum::' in s,
        'local_wrapper': lambda s: any(x in s for x in ('wc_match_wrapper', 'wc_like_wrapper', 'wc_regex_wrapper')),
        'callsite_or_cache': lambda s: '::matcher_callsite::' in s or '::matcher_cache::' in s,
        'matcher_engine': lambda s: bool(ENGINE.search(s)),
        'matcher_call': lambda s: bool(MATCHER_CALL.search(s)),
        'matcher_call_or_engine': lambda s: bool(MATCHER_CALL.search(s) or ENGINE.search(s)),
        'antalya_rs': lambda s: bool(re.search(r'antalya_rs(?:\[[^\]]+\])?::', s)),
        'arrow_ipc': lambda s: bool(re.search(r'arrow_ipc(?:\[[^\]]+\])?::', s)),
        'base64': lambda s: bool(re.search(r'base64(?:\[[^\]]+\])?::', s)),
        'unresolved_any': lambda s: '[unknown]' in s,
        'unresolved_extension': lambda s: any('pg_wildcard.so' in f and
             ('[unknown]' in f or re.match(r'^(?:0x)?[0-9a-f]+\s', f)) for f in s.split(';')),
    }
    total = sum(counts.values())
    inclusive = {name: sum(n for stack, n in counts.items() if rule(stack))
                 for name, rule in rules.items()}
    return dict(samples=total, inclusive_counts=inclusive,
                inclusive_percent={k: v / total * 100 for k, v in inclusive.items()},
                outside_visible_matcher_call_share=1 - inclusive['matcher_call'] / total,
                outside_engine_share=1 - inclusive['matcher_engine'] / total,
                non_matcher_sampled_share=1 - inclusive['matcher_call_or_engine'] / total)


def verify_stacks(directory):
    # Match the renderer's documented frame normalization, independently count
    # each perf sample, and require identical stacks rather than just totals.
    counts = Counter()
    frames = []
    with (directory / 'perf-demangled.script').open() as stream:
        for line in stream:
            if not line.strip():
                if frames:
                    counts[';'.join(reversed(frames))] += 1
                frames = []
                continue
            match = FRAME.match(line.rstrip('\n'))
            if match:
                frames.append(match[1].split('+0x')[0].strip() + ' [' + Path(match[2]).name + ']')
    if frames:
        counts[';'.join(reversed(frames))] += 1
    require(counts == folded_counts(directory / 'flamegraph.folded'),
            f'{directory}: folded stacks differ from demangled perf samples')
    svg = (directory / 'flamegraph.svg').read_text()
    require(f'CPU samples: {sum(counts.values())};' in svg, f'{directory}: SVG sample count differs')


def check_plan(result, workload):
    plan = result['plan']
    require(plan['Plan']['Actual Rows'] == rows_for(workload), f'{workload}: executor row count differs')
    require(plan['Plan'].get('Actual Loops', 1) == 1, f'{workload}: unexpected executor loops')
    number(result['executor_ms'], positive=True)
    require(math.isclose(result['executor_ms'], plan['Execution Time'], abs_tol=1e-9),
            f'{workload}: executor duration differs from plan')
    if workload == 'broad_recheck':
        def has_matcher(node):
            return ('wc_match(' in node.get('Filter', '') or
                    any(has_matcher(child) for child in node.get('Plans', [])))
        require(has_matcher(plan['Plan']), 'broad_recheck: local wc_match recheck absent from plan')


def build_report(root):
    build = read(root / 'build-provenance.json')
    sha = dict(baseline=build['baseline_binary_sha256'], optimized=build['candidate_binary_sha256'])
    for value in (*sha.values(), build['direct_worker_sha256']):
        require(bool(re.fullmatch('[0-9a-f]{64}', value)), 'invalid binary SHA256')
    require(sha['baseline'] != sha['optimized'], 'baseline and candidate binaries are identical')
    for label, name in (('baseline', 'baseline.so'), ('optimized', 'candidate.so')):
        if (root / name).exists():
            require(digest(root / name) == sha[label], f'{name}: actual binary hash differs')
    order = [f'{label}-{i}' for i in range(5)
             for label in (LABELS if i % 2 == 0 else tuple(reversed(LABELS)))]
    status = read(root / 'comparison-status.json')
    require([x['name'] for x in status] == order, 'cohorts were not recorded in the planned alternating order')
    for row in status:
        require(row['exit_status'] == 0 and row['binary_sha256'] == sha[row['name'].rsplit('-', 1)[0]],
                'unsuccessful cohort or status binary mismatch')
    values = {(w, r, l): [] for w in WORKLOADS for r in ROUTES for l in LABELS}
    executor = {(w, l): [] for w in WORKLOADS for l in LABELS}
    host_load = {label: [] for label in LABELS}
    invariant = None
    for i in range(5):
        for label in LABELS:
            directory = root / 'cohorts' / f'{label}-{i}'
            settings = read(directory / 'settings.json')
            require(settings['binary_sha256'] == sha[label], f'{directory}: cohort binary differs')
            require(settings['direct_worker_sha256'] == build['direct_worker_sha256'], 'direct worker changed')
            require(settings['label'] == label and settings['concurrency'] == 1 and settings['repeats'] == 1
                    and settings['policy'] == 'in_band' and settings['batch_bytes'] == 16777216
                    and settings['workloads'] == list(WORKLOADS) and settings['routes'] == list(ROUTES),
                    f'{directory}: workload or execution settings differ')
            same = {k: settings[k] for k in ('engine', 'runner_sha256', 'machine')}
            if invariant is None:
                invariant = same
            require(same == invariant, 'engine, runner, or machine settings changed between cohorts')
            require(read(directory / 'cleanup.json')['failures'] == [], f'{directory}: cleanup failed')
            require(not (directory / 'pending.json').exists(), f'{directory}: unfinished sample')
            records = [json.loads(line) for line in (directory / 'raw.jsonl').read_text().splitlines()]
            controls = [r for r in records if r['kind'] == 'value_control']
            queries = [r for r in records if r['kind'] == 'query']
            require(Counter(r['workload'] for r in controls) == Counter(WORKLOADS), 'missing/duplicate exact controls')
            require(Counter(r['workload'] for r in queries) == Counter(WORKLOADS), 'missing/duplicate query metadata')
            for query in queries:
                require(query['rows'] == rows_for(query['workload']), 'query expected rows differ')
            for control in controls:
                w = control['workload']
                check_copy(control['pg_copy'], w)
                require(control['rows'] == rows_for(w) and len(control['direct']) == 1, 'exact control shape differs')
                require(control['direct'][0]['value_control'] is True and
                        control['direct'][0]['rows'] == rows_for(w), 'missing direct exact-value check')
            samples = [r for r in records if r['kind'] == 'cohort']
            require(len(samples) == 24, f'{directory}: expected 12 warmup plus 12 measured samples')
            source_summary = read(directory / 'summary.json')
            require(len(source_summary) == 12 and len({(r['workload'], r['route']) for r in source_summary}) == 12,
                    f'{directory}: duplicate/missing source summaries')
            for w in WORKLOADS:
                for route in ROUTES:
                    group = [r for r in samples if r['workload'] == w and r['route'] == route]
                    require(len(group) == 2 and sorted(r['repeat'] for r in group) == [-1, 0], 'warmup/repeat shape differs')
                    for sample in group:
                        require(sample['warmup'] is (sample['repeat'] == -1), 'warmup flag differs')
                        require(sample['label'] == label and sample['requested_concurrency'] == 1
                                and len(sample['results']) == 1, 'cohort result shape differs')
                        result = sample['results'][0]
                        number(result['request_ms'], positive=True)
                        if route == 'direct':
                            require(result['rows'] == rows_for(w) and result['value_control'] is False,
                                    'direct timed sample row/verification mode differs')
                        elif route == 'pg_copy':
                            check_copy(result['value_control'], w)
                        else:
                            check_plan(result, w)
                        if not sample['warmup']:
                            load = sample['host_load']
                            require(isinstance(load, list) and len(load) == 3, 'invalid host-load observation')
                            host_load[label].append([number(value) for value in load])
                            values[w, route, label].append(result['request_ms'])
                            if route == 'pg_backend':
                                executor[w, label].append(result['executor_ms'])
                    summary = next(r for r in source_summary if r['workload'] == w and r['route'] == route)
                    expected = values[w, route, label][-1]
                    require(summary['samples'] == 1 and all(math.isclose(summary[k], expected, abs_tol=1e-9)
                            for k in ('median_ms', 'min_ms', 'max_ms')), 'source summary differs from raw samples')
                    if route == 'pg_backend':
                        require(math.isclose(summary['executor_median_ms'], executor[w, label][-1], abs_tol=1e-9),
                                'source executor summary differs')
    latency = []
    for w in WORKLOADS:
        for route in ROUTES:
            row = dict(workload=w, route=route,
                       request_ms={l: stats(values[w, route, l]) for l in LABELS},
                       request_change=paired_change(values[w, route, 'baseline'], values[w, route, 'optimized']))
            if route == 'pg_backend':
                row['executor_ms'] = {l: stats(executor[w, l]) for l in LABELS}
                row['executor_change'] = paired_change(executor[w, 'baseline'], executor[w, 'optimized'])
            latency.append(row)
    profiles = []
    profile_runner = None
    for label in LABELS:
        parent = root / 'flames' / label
        provenance = read(parent / 'provenance.json')
        require(provenance['label'] == label and provenance['binary_sha256'] == sha[label], 'profile binary differs')
        require(provenance['engine'] == invariant['engine'] and provenance['machine'] == invariant['machine'],
                'profile engine or machine differs from latency cohorts')
        require('--no-inline' in provenance['symbolization'], 'profile must record no synthetic inline expansion')
        profile_same = {k: provenance[k] for k in ('runner_sha256', 'postgres', 'engine_settings',
                                                  'symbolization', 'perf_version')}
        if profile_runner is None:
            profile_runner = profile_same
        require(profile_same == profile_runner, 'profile runner/PostgreSQL/engine settings/symbolization differ')
        require(read(parent / 'cleanup.json')['failures'] == [], 'profile cleanup failed')
        for w in WORKLOADS:
            directory = parent / w
            settings = read(directory / 'settings.json')
            require(settings['workload'] == w and settings['repetitions'] == 20
                    and settings['policy'] == 'in_band' and settings['batch_bytes'] == 16777216,
                    'profile settings differ')
            check_copy(settings['value_control'], w)
            plans = read(directory / 'plans.json')
            require(len(plans) == 20, 'profile must have twenty plans')
            for result in plans:
                check_plan(result, w)
            if (directory / 'provenance.json').exists():
                require(read(directory / 'provenance.json')['binary_sha256'] == sha[label], 'workload profile binary differs')
            cpu = read(directory / 'cpu.json')
            require(cpu['rows'] == rows_for(w) * 20, 'CPU row denominator differs')
            number(cpu['clock_ticks_per_second'], positive=True)
            user = number(cpu['user_seconds'], positive=True) * 1e6 / cpu['rows']
            system = number(cpu['system_seconds']) * 1e6 / cpu['rows']
            verify_stacks(directory)
            attribution = folded_attribution(directory / 'flamegraph.folded')
            profiles.append(dict(label=label, workload=w, rows=cpu['rows'], user_cpu_us_per_row=user,
                                 system_cpu_us_per_row=system, total_cpu_us_per_row=user + system,
                                 non_matcher_user_cpu_proxy_us_per_row=user * attribution['non_matcher_sampled_share'],
                                 attribution=attribution))
    target = []
    for w in WORKLOADS:
        before, after = [next(r for r in profiles if r['label'] == l and r['workload'] == w) for l in LABELS]
        ratio = after['non_matcher_user_cpu_proxy_us_per_row'] / number(before['non_matcher_user_cpu_proxy_us_per_row'], positive=True)
        target.append(dict(workload=w, optimized_over_baseline_non_matcher_cpu_proxy=ratio,
                           proxy_reduction_percent=(1 - ratio) * 100,
                           half_proxy_point_estimate_observed=ratio <= .5,
                           conclusion='single profile per binary; insufficient to establish halved true non-matcher overhead'))
    return dict(schema_version=1, verification='passed', cohort_order=order, binary_sha256=sha,
                direct_worker_sha256=build['direct_worker_sha256'],
                host_load=dict(scope='Host 1/5/15-minute load averages recorded after each measured route sample; warmups excluded; observations are correlated and are not CPU utilization.',
                               by_label={label: {name: stats([row[index] for row in host_load[label]])
                                                 for index, name in enumerate(('one_minute', 'five_minutes', 'fifteen_minutes'))}
                                         for label in LABELS}),
                settings=invariant, latency=latency, profiles=profiles, half_overhead_target=target,
                profile_symbolization={key: profile_runner[key] for key in ('symbolization', 'perf_version')},
                non_matcher_definition=DEFINITION,
                limitations=[
                    'Five alternating pairs; warmups excluded. Paired ratios use cohort index, not pooled medians. No significance claim.',
                    'Direct control consumes Arrow batches; PG executor is EXPLAIN ANALYZE TIMING OFF; COPY includes full binary delivery. These are different scopes.',
                    'Profile CPU includes profiling overhead and twenty scans. Do not compare it to unprofiled wall time as if clocks were interchangeable.',
                    'Sample categories are inclusive and overlap. Inlining, unresolved frames, and truncated stacks can hide matcher work.',
                    'Any-unresolved counts include system-library and PostgreSQL frames, not just extension frames. A stack counts once even with several unresolved frames; unresolved work cannot be assigned confidently.',
                    'Both profiles use perf --no-inline with exact saved ELF symbols and DWARF unwinding; no synthetic inline expansion. Resolved symbols alone cannot recover inlined Matcher::is_match boundaries.',
                    'Single CPU profile per workload/binary cannot establish a robust halving of true non-matcher overhead.',
                    'Shared-host background activity is not controlled by this report; host-load ranges describe recorded conditions, not causal attribution.',
                    'Exact-value controls are persisted runner attestations; raw COPY/Arrow payloads are not retained for independent value replay.',
                    'SHA consistency does not itself prove build reproducibility; source commits and flags are recorded in build-provenance.json.',
                ])


def markdown(report):
    lines = ['# FDW overhead evidence', '',
             'Five alternating baseline/optimized pairs, one measured observation per route and workload in each cohort. '
             'Warmups are excluded. Values below are median [minimum, maximum] across five observations.', '',
             '| Workload | Route / clock | Baseline ms | Optimized ms | Paired reduction median [range] |',
             '|---|---|---:|---:|---:|']
    def fmt(s):
        return f"{s['median']:.3f} [{s['min']:.3f}, {s['max']:.3f}]"
    for row in report['latency']:
        clock = 'direct complete Arrow' if row['route'] == 'direct' else 'COPY complete delivery' if row['route'] == 'pg_copy' else 'PG request round trip'
        lines.append(f"| {row['workload']} | {clock} | {fmt(row['request_ms']['baseline'])} | {fmt(row['request_ms']['optimized'])} | {fmt(row['request_change']['paired_reduction_percent'])}% |")
        if row['route'] == 'pg_backend':
            lines.append(f"| {row['workload']} | PG backend executor | {fmt(row['executor_ms']['baseline'])} | {fmt(row['executor_ms']['optimized'])} | {fmt(row['executor_change']['paired_reduction_percent'])}% |")
    lines += ['', 'The direct route is a distinct transport control; subtracting its wall clock from PG time does not isolate CPU overhead.', '',
              '| Workload | Binary | Samples | Any unresolved frame | User CPU µs/row | System CPU µs/row | Non-matcher sampled share | Non-matcher user CPU proxy µs/row |',
              '|---|---|---:|---:|---:|---:|---:|---:|']
    for row in report['profiles']:
        unknown = row['attribution']['inclusive_counts']['unresolved_any']
        unknown_percent = row['attribution']['inclusive_percent']['unresolved_any']
        lines.append(f"| {row['workload']} | {row['label']} | {row['attribution']['samples']} | {unknown} ({unknown_percent:.1f}%) | {row['user_cpu_us_per_row']:.4f} | {row['system_cpu_us_per_row']:.4f} | {100 * row['attribution']['non_matcher_sampled_share']:.1f}% | {row['non_matcher_user_cpu_proxy_us_per_row']:.4f} |")
    lines += ['', report['non_matcher_definition'], '',
              '| Workload | Non-matcher proxy reduction | At least 50% in point estimate? |',
              '|---|---:|---|']
    for row in report['half_overhead_target']:
        lines.append(f"| {row['workload']} | {row['proxy_reduction_percent']:.1f}% | {'yes' if row['half_proxy_point_estimate_observed'] else 'no'} |")
    lines += ['', '**The half-overhead target is not established by these single sampled profiles.** '
              'The table reports whether the defined proxy reached 50% in this observation; it does not establish halving of true non-matcher CPU.', '',
              'Verification checked complete cohort ordering, row counts and persisted exact-value controls, matching runner/settings/binary identities, '
              'source summaries against raw measurements, twenty plans per profile, and identical folded stacks reconstructed from demangled samples.', '',
              'Binary SHA256:', '']
    lines += [f"- {label}: `{sha}`" for label, sha in report['binary_sha256'].items()]
    lines += ['', 'Recorded host load (median [range]):', '',
              '| Binary | 1 minute | 5 minutes | 15 minutes |',
              '|---|---:|---:|---:|']
    for label in LABELS:
        loads = report['host_load']['by_label'][label]
        lines.append(f"| {label} | " + ' | '.join(fmt(loads[name]) for name in
                     ('one_minute', 'five_minutes', 'fifteen_minutes')) + ' |')
    lines += ['', report['host_load']['scope']]
    lines += ['', 'Limits:', ''] + ['- ' + item for item in report['limitations']]
    return '\n'.join(lines) + '\n'


def archive_manifest(root):
    # Positive allowlist only. No recursive whole-directory collection.
    names = ['build-provenance.json', 'comparison-status.json', 'fdw-benchmark.py',
             'fdw-flames-one.py', 'compare.py', 'report-overhead.py', 'test_report_overhead.py']
    for i in range(5):
        for label in LABELS:
            names += [f'cohorts/{label}-{i}/{name}' for name in
                      ('raw.jsonl', 'settings.json', 'summary.json', 'cleanup.json')]
    for label in LABELS:
        names += [f'flames/{label}/{name}' for name in ('provenance.json', 'cleanup.json')]
        for w in WORKLOADS:
            names += [f'flames/{label}/{w}/{name}' for name in
                      ('cpu.json', 'settings.json', 'plans.json', 'flamegraph.svg',
                       'flamegraph.folded', 'perf-demangled.script', 'perf-report.txt')]
            if (root / 'flames' / label / w / 'provenance.json').exists():
                names.append(f'flames/{label}/{w}/provenance.json')
    return [dict(path=name, bytes=(root / name).stat().st_size, sha256=digest(root / name))
            for name in sorted(names)]


TEMPLATE = '''# FDW overhead evidence (pending)

No measured result is available in this template. Generate the verified report:

```sh
python3 .local/followups-20261123/report-overhead.py --root .local/followups-20261123 --out .local/followups-20261123/evidence-report
```

Required input: build-provenance.json; comparison-status.json; five alternating
cohorts for each binary with raw.jsonl, settings.json, summary.json, cleanup.json;
both flame directories with provenance.json, cleanup.json and four workloads
containing cpu.json, settings.json, plans.json, flamegraph.svg,
flamegraph.folded, perf-demangled.script, perf-report.txt.

Outputs: deterministic summary.json, README.md, and archive-manifest.json.
The manifest is a positive allowlist of nonsecret evidence files with hashes.
Add the generated report files to a release archive, plus source runners and
their in-repository Python imports at the recorded source commit. Do not glob
the entire local directory. Exclude *.private.*, credentials, fixture state,
worker logs, raw perf.data, symlink symbol roots, build logs, and local bundles.
Saved baseline.so/candidate.so are optional separate symbolization assets;
the verifier hashes them when present, and always checks recorded binary hashes.
Review public raw data for secrets before publishing: allowlisting is not redaction.

The report separates direct Arrow transport, PG request round trip, PG executor,
and full COPY delivery. It reports process CPU per row separately from wall time.
Sample attribution is inclusive and can overlap. The non-matcher share is the
complement of stacks with visible Matcher::is_match or matcher-engine frames; inlining/unwinding
can hide work. A lower share alone does not prove a speedup or meet a target.
Both binaries use perf --no-inline with exact ELF symbols and DWARF unwinding;
there is no synthetic inline-frame expansion, so resolved symbols still omit
some inlined matcher boundaries. The verifier requires matching symbolization.
Half-overhead is assessed from the defined CPU proxy, never claimed in advance.
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--verify-only', action='store_true')
    parser.add_argument('--template', action='store_true')
    args = parser.parse_args()
    if args.template:
        print(TEMPLATE, end='')
        return
    try:
        report = build_report(args.root)
        manifest = archive_manifest(args.root)
        if args.verify_only:
            print('Evidence verified: 10 cohorts, 8 profiles; no performance conclusion implied.')
            return
        out = args.out or args.root / 'evidence-report'
        out.mkdir(parents=True, exist_ok=True)
        (out / 'summary.json').write_text(json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + '\n')
        (out / 'README.md').write_text(markdown(report))
        (out / 'archive-manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
        print(out)
    except (ValueError, KeyError, FileNotFoundError, StopIteration) as error:
        raise SystemExit(f'Evidence verification failed: {error}') from error


if __name__ == '__main__':
    main()
