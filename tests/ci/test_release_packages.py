# SPDX-License-Identifier: Apache-2.0
"""Publication boundaries for the two independently installable crates."""
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]

class ReleasePackages(unittest.TestCase):
    def test_extension_uses_publishable_canonical_dependency(self):
        manifest = tomllib.loads((ROOT / 'pg_wildcard/Cargo.toml').read_text())
        deps = manifest['dependencies']
        self.assertIn('libwildcard', deps)
        self.assertIn('hot', deps['libwildcard']['features'])
        self.assertFalse(set(deps) & {'wildcard-core', 'wildcard-ch', 'wildcard-puffin', 'wildcard-lake'})

    def test_extension_embedded_fixtures_are_inside_package(self):
        package = ROOT / 'pg_wildcard'
        for source in (package / 'src').rglob('*.rs'):
            for name in re.findall(r'include_(?:bytes|str)!\(\s*"([^"]+)"', source.read_text()):
                path = (source.parent / name).resolve()
                self.assertTrue(path.is_relative_to(package), str(source) + ': ' + name)
                self.assertTrue(path.is_file(), str(path))

    def test_release_metadata_and_notices_are_self_contained(self):
        for name in ('libwildcard', 'pg_wildcard'):
            package = ROOT / name
            manifest = tomllib.loads((package / 'Cargo.toml').read_text())['package']
            self.assertEqual(manifest['repository'], 'https://github.com/f1b0-gh/pg_wildcard')
            self.assertTrue((package / manifest['readme']).is_file())
            for file in ('LICENSE', 'NOTICE'):
                self.assertTrue((package / file).is_file(), name + '/' + file)

if __name__ == '__main__':
    unittest.main()
