# SPDX-License-Identifier: Apache-2.0
"""The guard measures Git objects, including binary additions and PR totals."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
RESULTS = 'tests/benchmarks/results/'


class ArtifactSize(unittest.TestCase):
    def setUp(self):
        script = ROOT / 'tools/check_artifact_size.py'
        self.assertTrue(script.exists(), 'artifact size guard has not been implemented')
        spec = importlib.util.spec_from_file_location('artifact_size', script)
        self.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.module)
        self.work = tempfile.TemporaryDirectory()
        self.addCleanup(self.work.cleanup)
        self.root = Path(self.work.name)
        self.git('init', '-q')
        self.git('config', 'user.name', 'Test')
        self.git('config', 'user.email', 'test@example.invalid')
        self.commit('base')
        self.base = self.git('rev-parse', 'HEAD').strip()

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args], text=True)

    def commit(self, name):
        self.git('add', '.')
        self.git('commit', '-qm', name, '--allow-empty')

    def write(self, name, size):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b'x' * size)

    def test_file_limit_is_inclusive_and_reads_git_not_worktree(self):
        name = 'tests/benchmarks/results/space and\nnewline.bin'
        self.write(name, 262144)
        self.commit('at limit')
        self.write(name, 262145)
        self.assertEqual(self.module.check(self.root, 'HEAD', self.base), [])
        self.commit('over limit')
        self.assertTrue(any('262145' in p for p in self.module.check(self.root, 'HEAD', self.base)))

    def test_pr_limit_cannot_be_offset_by_deletions(self):
        self.write(RESULTS + 'old.bin', 262144)
        self.commit('old')
        self.base = self.git('rev-parse', 'HEAD').strip()
        (self.root / RESULTS / 'old.bin').unlink()
        for number in range(8):
            self.write(RESULTS + f'new-{number}.bin', 262144)
        self.commit('at limit')
        self.assertEqual(self.module.check(self.root, 'HEAD', self.base), [])
        self.write(RESULTS + 'small.bin', 1)
        self.commit('over limit')
        self.assertTrue(any('2097153' in p for p in self.module.check(self.root, 'HEAD', self.base)))

    def test_modified_blob_counts_full_size_not_line_delta(self):
        for number in range(8):
            self.write(RESULTS + f'large-{number}.bin', 262144)
        self.commit('base file')
        self.base = self.git('rev-parse', 'HEAD').strip()
        for number in range(8):
            (self.root / RESULTS / f'large-{number}.bin').write_bytes(b'y' + b'x' * 262143)
        self.write(RESULTS + 'small.bin', 1)
        self.commit('one byte changed')
        self.assertTrue(any('2097153' in p for p in self.module.check(self.root, 'HEAD', self.base)))

    def test_unchanged_files_and_deletions_are_not_charged(self):
        for number in range(9):
            self.write(RESULTS + f'large-{number}.bin', 262144)
        self.commit('large baseline')
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.write(RESULTS + 'small.bin', 1)
        self.commit('small change')
        self.assertEqual(self.module.check(self.root, 'HEAD', self.base), [])
        (self.root / RESULTS / 'large-0.bin').unlink()
        self.commit('deletion')
        self.assertEqual(self.module.check(self.root, 'HEAD', self.base), [])

    def test_changes_outside_results_are_not_constrained(self):
        self.write('tests/fixtures/large.bin', 3 * 1024 * 1024)
        self.write('tests/benchmarks/results-other/large.bin', 3 * 1024 * 1024)
        self.write(RESULTS + 'small.bin', 1)
        self.commit('large unrelated fixtures')
        self.assertEqual(self.module.check(self.root, 'HEAD', self.base), [])


if __name__ == '__main__':
    unittest.main()
