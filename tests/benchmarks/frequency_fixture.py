#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Build real Parquet/Puffin file-frequency statistics for the sweep corpus.

Run on the host; only Parquet generation runs in the specified owned container.
Defaults write under .local/. No container or service lifecycle is modified.
"""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib


def corpus_value(row_id, indicators):
    if row_id % 100 == 0:
        return f"https://evil{(row_id // 100) % indicators:04}.example/path"
    return f"https://benign.example/path/{row_id}"


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def generate(args):
    import pyarrow as pa
    import pyarrow.parquet as pq

    out = args.output_dir
    out.mkdir(parents=True, exist_ok=True)
    if (out / "parquet-manifest.json").exists() or list(out.glob("part-*.parquet")):
        raise ValueError("output already contains a fixture; choose a fresh output directory")
    schema = pa.schema([("id", pa.int64()), ("v", pa.string())])
    source_digest = hashlib.sha256()
    observed = {gram: 0 for gram in [".ex", "vil", "ben", "l00", "l01", "000", "999"]}
    manifest = []
    hits = 0
    for ordinal in range(args.files):
        start = ordinal * args.rows // args.files
        end = (ordinal + 1) * args.rows // args.files
        ids = list(range(start, end))
        values = [corpus_value(i, args.indicators) for i in ids]
        for row_id, value in zip(ids, values):
            encoded = value.encode()
            source_digest.update(row_id.to_bytes(8, "big", signed=True))
            source_digest.update(len(encoded).to_bytes(8, "big"))
            source_digest.update(encoded)
            hits += row_id % 100 == 0
        for gram in observed:
            observed[gram] += any(gram in value for value in values)
        filename = f"part-{ordinal:05}.parquet"
        path = out / filename
        table = pa.Table.from_arrays([pa.array(ids, type=pa.int64()), pa.array(values, type=pa.string())], schema=schema)
        pq.write_table(table, path, compression="zstd", row_group_size=len(ids))
        loaded = pq.read_table(path)
        assert loaded.schema == schema and loaded.equals(table)
        manifest.append({"file": filename, "first_id": start, "last_id": end - 1,
                         "rows": len(ids), "bytes": path.stat().st_size, "sha256": file_hash(path)})
    assert sum(f["rows"] for f in manifest) == args.rows
    assert hits == (args.rows + 99) // 100
    write_json(out / "parquet-manifest.json", {
        "rows": args.rows, "files": args.files, "indicators": args.indicators,
        "schema": {"id": "Int64", "v": "String"}, "expected_indicator_hits": hits,
        "formula": "if id%100=0: https://evil{(id//100)%indicators:04}.example/path; else https://benign.example/path/{id}",
        "source_hash_encoding": "id signed i64 big endian, UTF-8 byte length u64 big endian, UTF-8 v; ascending id",
        "source_sha256": source_digest.hexdigest(), "pyarrow_version": pa.__version__,
        "known_interior_raw_gram_file_counts": observed, "parquet_files": manifest,
    })


def validate_export(out, paths, args):
    export = out / "export"
    metadata = json.loads((export / "gram-frequencies.json").read_text())
    with (export / "gram-frequencies.csv").open(newline="") as stream:
        records = list(csv.DictReader(stream))
    counts = {int(row["gram_key"]): int(row["file_count"]) for row in records}
    assert len(counts) == len(records) == metadata["gram_count"]
    assert all(1 <= count <= args.files for count in counts.values())
    assert all(key >> 27 == 0 for key in counts), "only raw-view grams expected"
    manifest = json.loads((out / "parquet-manifest.json").read_text())
    for gram, expected in manifest["known_interior_raw_gram_file_counts"].items():
        a, b, c = gram.encode()
        key = (a << 18) | (b << 9) | c
        assert counts.get(key, 0) == expected, (gram, counts.get(key), expected)
    coverage = hashlib.sha256(b"libwildcard/lake-coverage/v1\0")
    ordered_paths = sorted(set(str(path) for path in paths))
    coverage.update(len(ordered_paths).to_bytes(8, "big"))
    for path in ordered_paths:
        encoded = path.encode()
        coverage.update(len(encoded).to_bytes(8, "big"))
        coverage.update(encoded)
    assert metadata["coverage_id"] == "wc-lake-v1:" + coverage.hexdigest()
    assert metadata["column"] == "v" and metadata["views"] == ["raw"]
    assert metadata["snapshot_id"] == 1 and metadata["file_count"] == args.files
    assert metadata["files_with_unindexed_values"] == 0
    with (export / "files.csv").open(newline="") as stream:
        files = list(csv.reader(stream))
    assert len(files) == args.files and sum(int(row[3]) for row in files) == args.rows
    assert [row[2] for row in files] == [str(path) for path in paths]
    assert all(row[4] == "false" for row in files)
    with (export / "grams.csv").open(newline="") as stream:
        for row in csv.reader(stream):
            postings = [int(value) for value in row[2].strip("{}").split(",") if value]
            assert len(postings) == len(set(postings)) == counts[int(row[1])]
            assert all(0 <= ordinal < args.files for ordinal in postings)
    return metadata, counts


def main():
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=repo / ".local/perf-review/frequency")
    parser.add_argument("--rows", type=int, default=1_000_000)
    parser.add_argument("--files", type=int, default=100)
    parser.add_argument("--indicators", type=int, default=1000)
    parser.add_argument("--cli", type=Path, default=repo / ".local/antalya-registry/target/debug/wc-lake")
    parser.add_argument("--container", default="wildcard-review-pg18")
    parser.add_argument("--container-python", default="/workspace/.local/review-pg/venv/bin/python")
    parser.add_argument("--container-workspace", type=Path, default=Path("/workspace"))
    parser.add_argument("--generate-only", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not (1 <= args.files <= 128 and args.rows >= args.files and 1 <= args.indicators <= 10_000):
        parser.error("require 1 <= files <= 128, rows >= files, and 1 <= indicators <= 10000")
    args.output_dir = args.output_dir.resolve()
    if args.generate_only:
        generate(args)
        return
    out = args.output_dir
    relative_out = out.relative_to(repo)
    container_out = args.container_workspace / relative_out
    cli = args.cli.resolve(strict=True)
    out.mkdir(parents=True, exist_ok=True)
    generation = ["docker", "exec", "--user", f"{os.getuid()}:{os.getgid()}", args.container,
                  args.container_python, str(args.container_workspace / "tests/benchmarks/frequency_fixture.py"),
                  "--generate-only", "--output-dir", str(container_out), "--rows", str(args.rows),
                  "--files", str(args.files), "--indicators", str(args.indicators)]
    subprocess.run(generation, check=True)
    manifest = json.loads((out / "parquet-manifest.json").read_text())
    paths = [out / entry["file"] for entry in manifest["parquet_files"]]
    puffin = out / "benchmark-snapshot-1.puffin"
    index = [str(cli), "index", "--column", "v", "--views", "raw", "--snapshot-id", "1",
             "--out", str(puffin), *map(str, paths)]
    export = [str(cli), "export-pg", str(puffin), "--name", "benchmark", "--out-dir", str(out / "export")]
    for name, command in [("index", index), ("export", export), ("inspect", [str(cli), "inspect", str(puffin)])]:
        with (out / f"{name}.log").open("w") as log:
            subprocess.run(command, check=True, stdout=log, stderr=subprocess.STDOUT)
    metadata, counts = validate_export(out, paths, args)
    probe = subprocess.run([str(cli), "--version"], text=True, capture_output=True)
    package = tomllib.loads((repo / "libwildcard/Cargo.toml").read_text())["package"]
    record = {
        "rows": args.rows, "files": args.files, "indicators": args.indicators,
        "source_sha256": manifest["source_sha256"], "source_hash_encoding": manifest["source_hash_encoding"],
        "cli": str(cli), "cli_sha256": file_hash(cli),
        "cli_version_probe": {"exit_code": probe.returncode, "stdout": probe.stdout, "stderr": probe.stderr},
        "declared_package_version": package["version"],
        "version_note": "CLI lacks a version command; package version describes source, binary SHA-256 identifies executable",
        "puffin": str(puffin), "puffin_sha256": file_hash(puffin), "puffin_bytes": puffin.stat().st_size,
        "commands": {"generate": generation, "index": index, "export": export},
        "frequency_metadata": metadata, "min_file_count": min(counts.values()), "max_file_count": max(counts.values()),
        "known_interior_raw_gram_file_counts": manifest["known_interior_raw_gram_file_counts"],
        "frequency_csv": str(out / "export/gram-frequencies.csv"),
        "frequency_csv_sha256": file_hash(out / "export/gram-frequencies.csv"),
        "validation": "all exported postings agree with counts; complete inventory coverage, row totals, schema, source Parquet round-trip and known gram counts checked",
    }
    write_json(out / "validation.json", record)
    print(json.dumps({key: record[key] for key in ["frequency_csv", "source_sha256", "puffin_bytes", "min_file_count", "max_file_count"]}))


if __name__ == "__main__":
    main()
