# SPDX-License-Identifier: Apache-2.0
import importlib.util
from pathlib import Path
import unittest

SOURCE = Path(__file__).resolve().parents[2] / "tools/check_dependency_policy.py"
spec = importlib.util.spec_from_file_location("dependency_policy", SOURCE)
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


class DependencyPolicyTests(unittest.TestCase):
    def test_library_consumers_need_exact_manifest_pins(self):
        self.assertEqual(policy.check_manifest({"dependencies": {"arrow-array": "=59.3.3"}}), [])
        self.assertTrue(policy.check_manifest({"dependencies": {"arrow-array": "59.3.3"}}))
        self.assertTrue(policy.check_manifest({"dependencies": {"parquet": {"version": "=60.0.0"}}}))

    def test_antalya_release_requires_an_exact_version(self):
        self.assertEqual(policy.check_manifest({"dependencies": {
            "antalya-rs": {"version": "=0.1.1", "features": ["tls"]}
        }}), [])
        self.assertTrue(policy.check_manifest({"dependencies": {
            "antalya-rs": {"version": "0.1.1"}
        }}))
        self.assertTrue(policy.check_manifest({"dependencies": {
            "antalya-rs": {"path": "../antalya-rs"}
        }}))
        self.assertTrue(policy.check_lock({"package": [
            {"name": "antalya-rs", "version": "0.2.0"}
        ]}))

    def test_target_dev_aliases_and_workspace_inheritance_are_checked(self):
        manifest = {"target": {"cfg(unix)": {"dev-dependencies": {
            "renamed": {"package": "arrow-array", "version": "=60.0.0"}
        }}}}
        self.assertTrue(policy.check_manifest(manifest))
        manifest = {"dependencies": {"arrow-array": {"workspace": True}}}
        self.assertTrue(policy.check_manifest(manifest))
        self.assertEqual(policy.check_manifest(manifest, {"arrow-array": "=59.3.3"}), [])

    def test_mixed_transitive_arrow_versions_are_rejected(self):
        packages = [{"name": "arrow-array", "version": "59.3.3"},
                    {"name": "arrow-buffer", "version": "60.0.0"}]
        self.assertTrue(policy.check_lock({"package": packages}, require_arrow=True))
        self.assertTrue(policy.check_lock({"package": []}, require_arrow=True))

    def test_antalya_and_clickhouse_versions_are_checked_together(self):
        packages = [{"name": name, "version": version}
                    for name, version in policy.COUPLED.items()]
        self.assertEqual(policy.check_lock({"package": packages}, require_client=True), [])
        packages.append({"name": "clickhouse", "version": "0.16.0"})
        self.assertTrue(policy.check_lock({"package": packages}, require_client=True))
        self.assertTrue(policy.check_lock({"package": []}, require_client=True))


if __name__ == "__main__":
    unittest.main()
