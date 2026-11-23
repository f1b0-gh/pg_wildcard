# SPDX-License-Identifier: Apache-2.0
"""Exercise archive integrity and extraction without a network dependency."""
import gzip
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


class Artifacts(unittest.TestCase):
    def setUp(self):
        self.work = tempfile.TemporaryDirectory()
        self.addCleanup(self.work.cleanup)
        self.root = Path(self.work.name)
        script = ROOT / 'tools/verify_benchmark_artifacts.py'
        self.assertTrue(script.exists(), 'archive verifier has not been implemented')
        spec = importlib.util.spec_from_file_location('artifacts', script)
        self.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.module)

    def archive(self, entries=None):
        entries = entries if entries is not None else [('tests/benchmarks/results/a.txt', b'evidence', None)]
        raw = io.BytesIO()
        with tarfile.open(fileobj=raw, mode='w', format=tarfile.USTAR_FORMAT) as tar:
            for name, data, kind in entries:
                info = tarfile.TarInfo(name)
                info.size = len(data)
                if kind:
                    info.type = kind
                    info.linkname = '/tmp/escape'
                tar.addfile(info, io.BytesIO(data))
        payload = gzip.compress(raw.getvalue(), mtime=0)
        source = self.root / 'source.tar.gz'
        source.write_bytes(payload)
        manifest = dict(sha256=hashlib.sha256(payload).hexdigest(), size=len(payload),
                        tar_size=len(raw.getvalue()), files=len(entries),
                        unpacked_size=sum(len(data) for _, data, _ in entries),
                        url='https://example.invalid/evidence.tar.gz')
        return source, manifest

    def test_local_import_is_verified_and_reused_offline(self):
        source, manifest = self.archive()
        cached = self.module.ensure_archive(manifest, self.root / 'cache', source, True)
        source.unlink()
        self.assertEqual(cached.read_bytes()[:2], b'\x1f\x8b')
        self.assertEqual(self.module.ensure_archive(manifest, self.root / 'cache', None, True), cached)

    def test_corruption_never_becomes_a_valid_cache_entry(self):
        source, manifest = self.archive()
        source.write_bytes(b'wrong checksum')
        with self.assertRaisesRegex(ValueError, 'size|checksum'):
            self.module.ensure_archive(manifest, self.root / 'cache', source, True)
        self.assertFalse(list((self.root / 'cache').glob('*.tar.gz')))

    def test_offline_missing_or_corrupt_cache_is_actionable(self):
        source, manifest = self.archive()
        with self.assertRaisesRegex(ValueError, 'offline|--archive'):
            self.module.ensure_archive(manifest, self.root / 'cache', None, True)
        cached = self.module.ensure_archive(manifest, self.root / 'cache', source, True)
        cached.write_bytes(b'bad')
        with self.assertRaisesRegex(ValueError, 'checksum|size|--archive'):
            self.module.ensure_archive(manifest, self.root / 'cache', None, True)

    def test_exact_payload_extracted(self):
        source, manifest = self.archive()
        dest = self.root / 'unpacked'
        self.module.extract_archive(source, dest, manifest)
        self.assertEqual((dest / 'tests/benchmarks/results/a.txt').read_bytes(), b'evidence')

    def test_unsafe_paths_links_and_duplicates_rejected(self):
        for name, kind in [('../escape', None), ('/tmp/escape', None),
                           ('tests/benchmarks/results/../escape', None),
                           ('tests/benchmarks/results//a', None),
                           ('tests/benchmarks/results/link', tarfile.SYMTYPE),
                           ('tests/benchmarks/results/link', tarfile.LNKTYPE),
                           ('tests/benchmarks/results/device', tarfile.CHRTYPE),
                           ('other/file', None)]:
            with self.subTest(name=name, kind=kind):
                source, manifest = self.archive([(name, b'', kind)])
                with tempfile.TemporaryDirectory(dir=self.root) as dest:
                    with self.assertRaises(ValueError):
                        self.module.extract_archive(source, Path(dest), manifest)
        source, manifest = self.archive([('tests/benchmarks/results/a', b'1', None)] * 2)
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            self.module.extract_archive(source, self.root / 'duplicate', manifest)

    def test_decompression_payload_and_member_count_limits(self):
        source, original = self.archive()
        for field in ('tar_size', 'unpacked_size', 'files'):
            with self.subTest(field=field):
                manifest = dict(original)
                manifest[field] -= 1
                with self.assertRaises(ValueError):
                    self.module.extract_archive(source, self.root / field, manifest)

    def test_existing_destination_is_not_overwritten(self):
        source, manifest = self.archive()
        dest = self.root / 'unpacked'
        dest.mkdir()
        (dest / 'precious').write_text('keep')
        with self.assertRaises(ValueError):
            self.module.extract_archive(source, dest, manifest)
        self.assertEqual((dest / 'precious').read_text(), 'keep')

    def test_archive_digest_rechecked_before_extraction(self):
        source, manifest = self.archive()
        manifest['sha256'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'checksum'):
            self.module.extract_archive(source, self.root / 'unpacked', manifest)

    def test_failed_network_transfer_does_not_replace_cache(self):
        source, manifest = self.archive()
        class BrokenTransfer(io.BytesIO):
            url = 'https://example.invalid/evidence.tar.gz'
            def read(self, size=-1):
                if self.tell():
                    raise OSError('connection lost')
                return super().read(10)
        with patch.object(self.module.urllib.request, 'urlopen', return_value=BrokenTransfer(source.read_bytes())):
            with self.assertRaisesRegex(OSError, 'connection lost'):
                self.module.ensure_archive(manifest, self.root / 'cache')
        self.assertEqual(list((self.root / 'cache').iterdir()), [])

    def test_manifest_verifier_runs_in_disposable_checkout(self):
        source, manifest = self.archive([('tests/benchmarks/results/run/verify.py',
            b'from pathlib import Path\nPath("receipt").write_text("verified")\n', None)])
        dest = self.root / 'unpacked'
        self.module.extract_archive(source, dest, manifest)
        manifest['verifiers'] = ['tests/benchmarks/results/run/verify.py']
        self.module.verify_reports(dest, manifest)
        self.assertEqual((dest / 'receipt').read_text(), 'verified')

    def test_manifest_verifier_cannot_escape_archive(self):
        _, manifest = self.archive()
        manifest['verifiers'] = ['../../outside.py']
        with self.assertRaisesRegex(ValueError, 'verifier'):
            self.module.validate_manifest(manifest)

    def test_keep_preserves_only_successfully_verified_evidence(self):
        source, manifest = self.archive([('tests/benchmarks/results/run/verify.py', b'raise SystemExit(0)\n', None)])
        manifest['verifiers'] = ['tests/benchmarks/results/run/verify.py']
        path = self.root / 'manifest.json'
        path.write_text(json.dumps(manifest))
        args = ['verify', '--manifest', str(path), '--archive', str(source), '--offline', '--keep']
        with patch.object(self.module, 'ROOT', self.root), patch.object(self.module.sys, 'argv', args):
            self.assertEqual(self.module.main(), 0)
        retained = list((self.root / '.local/benchmark-artifacts').glob('verified-*'))
        self.assertEqual(len(retained), 1)
        self.assertTrue((retained[0] / 'tests/benchmarks/results/run/verify.py').is_file())

    def test_failing_verifier_does_not_retain_an_approved_directory(self):
        source, manifest = self.archive([('tests/benchmarks/results/run/verify.py', b'raise SystemExit(1)\n', None)])
        manifest['verifiers'] = ['tests/benchmarks/results/run/verify.py']
        path = self.root / 'manifest.json'
        path.write_text(json.dumps(manifest))
        args = ['verify', '--manifest', str(path), '--archive', str(source), '--offline', '--keep']
        with patch.object(self.module, 'ROOT', self.root), patch.object(self.module.sys, 'argv', args):
            self.assertEqual(self.module.main(), 1)
        self.assertEqual(list((self.root / '.local/benchmark-artifacts').glob('verified-*')), [])
        self.assertEqual(list((self.root / '.local/benchmark-artifacts').glob('verify-*')), [])

    def test_malformed_manifest_is_a_bounded_validation_error(self):
        _, manifest = self.archive()
        for value in [[], {**manifest, 'sha256': 123}, {**manifest, 'url': None},
                      {**manifest, 'size': True}, {**manifest, 'tar_size': 1 << 40},
                      {**manifest, 'schema_version': 2}]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.module.validate_manifest(value)


if __name__ == '__main__':
    unittest.main()
