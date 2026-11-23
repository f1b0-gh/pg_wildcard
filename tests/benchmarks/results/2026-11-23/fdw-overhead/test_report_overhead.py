# SPDX-License-Identifier: Apache-2.0
"""Synthetic evidence checks; does not run PostgreSQL or a benchmark."""
import hashlib
import importlib.util
import json
import struct
import tempfile
import unittest
from pathlib import Path

PATH = Path(__file__).with_name('report-overhead.py')


def fixture_copy_size(workload):
    """Compute size from encoded COPY fields, independently of verifier totals."""
    rows = 32768 if workload == 'wide1k' else 1000000
    identifier = struct.pack('!q', 1)
    def encoded_row(url):
        fields = (identifier,) if workload == 'narrow' else (
            identifier, url, b'x' * (1024 if workload == 'wide1k' else 16))
        return struct.pack('!h', len(fields)) + b''.join(
            struct.pack('!i', len(field)) + field for field in fields)
    header = b'PGCOPY\n\xff\r\n\0' + struct.pack('!ii', 0, 0)
    trailer = struct.pack('!h', -1)
    marker_rows = rows // 10000 if workload in ('smalltext', 'broad_recheck') else 0
    return (len(header) + len(trailer)
            + (rows - marker_rows) * len(encoded_row(b'benign.test/normal'))
            + marker_rows * len(encoded_row(b'evil.test/powershell')))


def fixture(root):
    """Hand-sized artifact set with warmups deliberately far above measured times."""
    def write(path, value):
        file = root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(json.dumps(value))
    workloads = ('narrow', 'smalltext', 'wide1k', 'broad_recheck')
    routes = ('direct', 'pg_backend', 'pg_copy')
    labels = ('baseline', 'optimized')
    write('build-provenance.json', dict(baseline_binary_sha256='a' * 64,
          candidate_binary_sha256='b' * 64, direct_worker_sha256='c' * 64))
    order = [f'{label}-{i}' for i in range(5) for label in (labels if i % 2 == 0 else labels[::-1])]
    write('comparison-status.json', [dict(name=name, exit_status=0,
          binary_sha256=('a' if name.startswith('baseline') else 'b') * 64) for name in order])
    sizes = {workload: fixture_copy_size(workload) for workload in workloads}
    for label, sha, duration in (('baseline', 'a' * 64, 100), ('optimized', 'b' * 64, 75)):
        for i in range(5):
            prefix = f'cohorts/{label}-{i}'
            write(prefix + '/settings.json', dict(label=label, binary_sha256=sha,
                  direct_worker_sha256='c' * 64, concurrency=1, repeats=1, policy='in_band',
                  batch_bytes=16777216, workloads=list(workloads), routes=list(routes),
                  engine='test', runner_sha256='d' * 64, machine={}))
            write(prefix + '/cleanup.json', dict(failures=[]))
            records, summaries = [], []
            for workload in workloads:
                rows = 32768 if workload == 'wide1k' else 1000000
                control = dict(rows=rows, copy_bytes=sizes[workload], oracle='every field and complete unique row-ID set')
                records += [dict(kind='query', workload=workload, rows=rows),
                            dict(kind='value_control', workload=workload, rows=rows,
                                 direct=[dict(value_control=True, rows=rows)], pg_copy=control)]
                for route in routes:
                    for repeat, value in ((-1, 10000), (0, duration + i)):
                        result = dict(request_ms=value, rows=rows,
                                      value_control=control if route == 'pg_copy' else False)
                        if route == 'pg_backend':
                            result.update(executor_ms=value - 10,
                                          plan={'Plan': {'Actual Rows': rows, 'Actual Loops': 1}, 'Execution Time': value - 10})
                            if workload == 'broad_recheck':
                                result['plan']['Plan']['Filter'] = "wc_match(url, '*normal*', 'raw') OR wc_match(url, '*powershell*', 'raw')"
                        records.append(dict(kind='cohort', workload=workload, route=route,
                                            repeat=repeat, warmup=repeat == -1, label=label,
                                            requested_concurrency=1, results=[result],
                                            host_load=[99, 99, 99] if repeat == -1 else [i + 1, i + 2, i + 3]))
                    summaries.append(dict(workload=workload, route=route, samples=1,
                                          median_ms=duration + i, min_ms=duration + i, max_ms=duration + i,
                                          executor_median_ms=duration + i - 10 if route == 'pg_backend' else None))
            (root / prefix / 'raw.jsonl').write_text(''.join(json.dumps(r) + '\n' for r in records))
            write(prefix + '/summary.json', summaries)
        prefix = f'flames/{label}'
        write(prefix + '/provenance.json', dict(label=label, binary_sha256=sha, engine='test',
              machine={}, runner_sha256='e' * 64, postgres='18', engine_settings={},
              symbolization='perf --no-inline with exact saved ELF: real DWARF-unwound frames; no synthetic inline expansion',
              perf_version='test'))
        write(prefix + '/cleanup.json', dict(failures=[]))
        for workload in workloads:
            rows = 32768 if workload == 'wide1k' else 1000000
            directory = f'{prefix}/{workload}'
            write(directory + '/cpu.json', dict(rows=rows * 20, user_seconds=2 if label == 'baseline' else 1.5,
                  system_seconds=.1, clock_ticks_per_second=100))
            write(directory + '/settings.json', dict(workload=workload, repetitions=20,
                  policy='in_band', batch_bytes=16777216,
                  value_control=dict(rows=rows, copy_bytes=sizes[workload], oracle='every field and complete unique row-ID set')))
            plan = {'Plan': {'Actual Rows': rows}, 'Execution Time': 100}
            if workload == 'broad_recheck':
                plan['Plan']['Filter'] = "wc_match(url, '*normal*', 'raw') OR wc_match(url, '*powershell*', 'raw')"
            write(directory + '/plans.json', [dict(executor_ms=100, plan=plan) for _ in range(20)])
            (root / directory / 'perf-demangled.script').write_text('postgres\n    a pg_wildcard::fdw::iterate_foreign_scan+0x1 (/lib/pg_wildcard.so)\n\n')
            (root / directory / 'flamegraph.folded').write_text('pg_wildcard::fdw::iterate_foreign_scan [pg_wildcard.so] 1\n')
            (root / directory / 'flamegraph.svg').write_text('<svg>CPU samples: 1;</svg>')


class EvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location('report_overhead', PATH)
        cls.report = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.report)

    def test_pairwise_reduction_is_not_ratio_of_pooled_medians(self):
        # Paired ratios are .5, .25, 1; their median is .5, not 50/200.
        result = self.report.paired_change([100, 200, 1000], [50, 50, 1000])
        self.assertEqual(result['paired_optimized_over_baseline']['median'], .5)
        self.assertEqual(result['paired_reduction_percent']['min'], 0)
        self.assertEqual(result['paired_reduction_percent']['max'], 75)
        self.assertFalse(result['every_pair_at_least_half'])

    def test_invalid_denominator_cannot_produce_success_claim(self):
        with self.assertRaises(ValueError):
            self.report.paired_change([0], [0])

    def test_stack_categories_overlap_and_only_engine_is_removed(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'flamegraph.folded'
            path.write_text('postgres;pg_wildcard::fdw::iterate_foreign_scan;arrow_ipc::decode 3\n'
                            'postgres;wc_match_wrapper;regex_automata::search 2\n'
                            'postgres;wc_match_wrapper;pg_wildcard::matcher_cache::get 5\n')
            result = self.report.folded_attribution(path)
        self.assertEqual(result['samples'], 10)
        self.assertEqual(result['non_matcher_sampled_share'], .8)
        self.assertEqual(result['inclusive_counts']['local_wrapper'], 7)
        self.assertEqual(result['inclusive_counts']['arrow_ipc'], 3)

    def test_whole_matcher_removes_view_work_below_visible_is_match(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'flamegraph.folded'
            for symbol in ('libwildcard::core::verify::Matcher::is_match',
                           '<libwildcard[9a5910dac72784dd]::core::verify::Matcher>::is_match'):
                with self.subTest(symbol=symbol):
                    path.write_text(f'wc_match_wrapper;{symbol};libwildcard::core::views::View::apply 3\n'
                                    'wc_match_wrapper;regex_automata::search 2\n'
                                    'wc_match_wrapper;pg_wildcard::matcher_cache::get 5\n')
                    result = self.report.folded_attribution(path)
                    self.assertEqual(result['non_matcher_sampled_share'], .5)
                    self.assertEqual(result['outside_visible_matcher_call_share'], .7)
                    self.assertEqual(result['outside_engine_share'], .8)

    def test_unknown_counts_include_system_frames_without_double_counting(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'flamegraph.folded'
            path.write_text('[unknown] [postgres];[unknown] [libc.so.6] 3\n'
                            '[unknown] [pg_wildcard.so];wc_match_wrapper 2\n'
                            'postgres;wc_match_wrapper 5\n')
            result = self.report.folded_attribution(path)
        self.assertEqual(result['inclusive_counts']['unresolved_any'], 5)
        self.assertEqual(result['inclusive_counts']['unresolved_extension'], 2)
        self.assertEqual(result['inclusive_percent']['unresolved_any'], 50)

    def test_copy_control_rejects_count_only_or_bad_size(self):
        with self.assertRaises(ValueError):
            self.report.check_copy({'rows': 1000000}, 'narrow')
        with self.assertRaises(ValueError):
            self.report.check_copy({'rows': 1000000, 'copy_bytes': 1,
                                    'oracle': 'every field and complete unique row-ID set'}, 'narrow')
        self.report.check_copy({'rows': 1000000, 'copy_bytes': 14000021,
                                'oracle': 'every field and complete unique row-ID set'}, 'narrow')

    def test_copy_controls_match_sizes_of_actual_encoded_fields(self):
        for workload in ('narrow', 'smalltext', 'wide1k', 'broad_recheck'):
            with self.subTest(workload=workload):
                self.report.check_copy(dict(rows=32768 if workload == 'wide1k' else 1000000,
                                            copy_bytes=fixture_copy_size(workload),
                                            oracle='every field and complete unique row-ID set'), workload)

    def test_missing_inputs_fail_without_emitting_a_report(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaises((ValueError, FileNotFoundError)):
                self.report.build_report(Path(temporary))

    def test_complete_evidence_excludes_warmups_and_preserves_clock_scopes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture(root)
            result = self.report.build_report(root)
            direct, backend = result['latency'][:2]
            self.assertEqual(direct['request_ms']['baseline'], dict(n=5, median=102, min=100, max=104))
            self.assertEqual(backend['executor_ms']['optimized']['median'], 67)
            self.assertAlmostEqual(result['profiles'][0]['user_cpu_us_per_row'], .1)
            self.assertAlmostEqual(result['half_overhead_target'][0]['proxy_reduction_percent'], 25)
            self.assertFalse(result['half_overhead_target'][0]['half_proxy_point_estimate_observed'])
            self.assertEqual(result['host_load']['by_label']['baseline']['one_minute'],
                             dict(n=60, median=3, min=1, max=5))
            self.assertEqual(result, self.report.build_report(root))
            self.assertIn('not established', self.report.markdown(result))

    def test_optional_binary_presence_does_not_change_report_but_invalid_binary_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture(root)
            binary = b'baseline fixture'
            binary_sha = hashlib.sha256(binary).hexdigest()
            for path in root.rglob('*.json'):
                path.write_text(path.read_text().replace('a' * 64, binary_sha))
            absent = self.report.build_report(root)
            (root / 'baseline.so').write_bytes(binary)
            self.assertEqual(absent, self.report.build_report(root))
            (root / 'baseline.so').write_bytes(b'incorrect binary')
            with self.assertRaisesRegex(ValueError, 'actual binary hash differs'):
                self.report.build_report(root)

    def test_binary_mismatch_and_missing_direct_control_fail_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture(root)
            path = root / 'cohorts/optimized-4/settings.json'
            settings = json.loads(path.read_text())
            settings['binary_sha256'] = 'a' * 64
            path.write_text(json.dumps(settings))
            with self.assertRaisesRegex(ValueError, 'binary differs'):
                self.report.build_report(root)
            fixture(root)
            path = root / 'cohorts/baseline-0/raw.jsonl'
            records = [json.loads(line) for line in path.read_text().splitlines()]
            next(r for r in records if r['kind'] == 'value_control')['direct'][0]['value_control'] = False
            path.write_text(''.join(json.dumps(r) + '\n' for r in records))
            with self.assertRaisesRegex(ValueError, 'direct exact-value'):
                self.report.build_report(root)

    def test_stack_or_cpu_denominator_tampering_fails_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture(root)
            path = root / 'flames/baseline/narrow/flamegraph.folded'
            path.write_text('pg_wildcard::fdw::iterate_foreign_scan [pg_wildcard.so] 2\n')
            with self.assertRaisesRegex(ValueError, 'folded stacks differ'):
                self.report.build_report(root)
            fixture(root)
            path = root / 'flames/baseline/narrow/cpu.json'
            cpu = json.loads(path.read_text())
            cpu['rows'] = 1000000
            path.write_text(json.dumps(cpu))
            with self.assertRaisesRegex(ValueError, 'CPU row denominator'):
                self.report.build_report(root)

    def test_broad_recheck_must_actually_keep_local_matcher_filter(self):
        result = {'executor_ms': 100, 'plan': {'Execution Time': 100,
                  'Plan': {'Actual Rows': 1000000, 'Filter': "url ~~ '%normal%'"}}}
        with self.assertRaisesRegex(ValueError, 'local wc_match recheck'):
            self.report.check_plan(result, 'broad_recheck')

    def test_profile_symbolization_must_match_between_binaries(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture(root)
            path = root / 'flames/optimized/provenance.json'
            provenance = json.loads(path.read_text())
            provenance['perf_version'] = 'different'
            path.write_text(json.dumps(provenance))
            with self.assertRaisesRegex(ValueError, 'profile runner/PostgreSQL/engine settings/symbolization differ'):
                self.report.build_report(root)


if __name__ == '__main__':
    unittest.main()
