#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Check the qualified Arrow/Parquet/ClickHouse set without resolving Cargo paths."""
import argparse
from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parents[1]
ARROW_VERSION = "59.3.3"
COUPLED = {"clickhouse": "0.15.3", "clickhouse-ext-arrow": "0.2.0"}


def expected_version(name):
    if name.startswith("arrow-") or name == "parquet":
        return ARROW_VERSION
    return COUPLED.get(name)


def dependency_tables(table):
    for key, value in table.items():
        if not isinstance(value, dict):
            continue
        if key in {"dependencies", "dev-dependencies", "build-dependencies"}:
            yield value
        else:
            yield from dependency_tables(value)


def check_manifest(manifest, workspace=None):
    errors = []
    workspace = workspace or manifest.get("workspace", {}).get("dependencies", {})
    for table in dependency_tables(manifest):
        for alias, spec in table.items():
            if isinstance(spec, dict) and spec.get("workspace"):
                spec = workspace.get(alias, {})
            name = spec.get("package", alias) if isinstance(spec, dict) else alias
            expected = expected_version(name)
            if expected is None:
                continue
            actual = spec.get("version") if isinstance(spec, dict) else spec
            if actual != f"={expected}":
                errors.append(f"{name}: expected exact ={expected} manifest pin, found {actual!r}")
    return errors


def check_lock(lock, *, require_arrow=False, require_client=False):
    errors = []
    seen = set()
    for package in lock.get("package", []):
        name = package["name"]
        expected = expected_version(name)
        if expected is None:
            continue
        seen.add(name)
        if package["version"] != expected:
            errors.append(f"{name}: lock resolves {package['version']}, expected {expected}")
    if require_arrow and not any(name.startswith("arrow-") for name in seen):
        errors.append("lockfile has no Arrow packages")
    if require_client:
        for name in COUPLED.keys() - seen:
            errors.append(f"lockfile is missing {name}")
    return errors


def read(path):
    with path.open("rb") as source:
        return tomllib.load(source)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--companion", type=Path, help="also validate an explicit antalya-rs checkout")
    args = parser.parse_args()
    workspace = read(ROOT / "Cargo.toml")["workspace"]["dependencies"]
    manifests = [ROOT / "Cargo.toml", ROOT / "libwildcard/Cargo.toml",
                 ROOT / "pg_wildcard/Cargo.toml", ROOT / "tests/transport/Cargo.toml",
                 ROOT / "tests/fdw/harness/Cargo.toml", ROOT / "fuzz/Cargo.toml",
                 *sorted(ROOT.glob("crates/*/Cargo.toml"))]
    locks = [(ROOT / "Cargo.lock", True, True),
             (ROOT / "libwildcard/Cargo.lock", True, False),
             (ROOT / "tests/transport/Cargo.lock", True, True),
             (ROOT / "tests/fdw/harness/Cargo.lock", True, True),
             (ROOT / "fuzz/Cargo.lock", False, False)]
    if args.companion:
        manifests.append(args.companion / "Cargo.toml")
        locks.append((args.companion / "Cargo.lock", True, True))
    errors = []
    for path in manifests:
        for error in check_manifest(read(path), workspace):
            errors.append(f"{path.relative_to(ROOT) if path.is_relative_to(ROOT) else path}: {error}")
    for path, arrow, client in locks:
        for error in check_lock(read(path), require_arrow=arrow, require_client=client):
            errors.append(f"{path.relative_to(ROOT) if path.is_relative_to(ROOT) else path}: {error}")
    if errors:
        parser.exit(1, "\n".join(errors) + "\n")
    print(f"Dependency policy passed: {len(manifests)} manifests, {len(locks)} lockfiles; "
          f"Arrow/Parquet {ARROW_VERSION}, ClickHouse {COUPLED['clickhouse']}, "
          f"adapter {COUPLED['clickhouse-ext-arrow']}.")


if __name__ == "__main__":
    main()
