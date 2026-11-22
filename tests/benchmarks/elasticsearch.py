#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Owned Elasticsearch fixture: literal-pair sweeps and separately labelled index costs.

Reads public HTTP APIs only. No Elasticsearch implementation source/dependencies.
Setup: --phase setup. Timed searches: --phase queries (coordinate CPU use).
The private state is produced by fixtures.py; never publish it with results.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import time
import urllib.error
import urllib.request

DOCS = [
    "https://www.elastic.co/docs/reference/elasticsearch/mapping-reference/keyword#wildcard-field-type",
    "https://www.elastic.co/docs/reference/query-languages/query-dsl/query-dsl-wildcard-query",
    "https://www.elastic.co/docs/reference/query-languages/query-dsl/query-dsl-regexp-query",
    "https://www.elastic.co/docs/api/doc/elasticsearch/operation/operation-msearch",
    "https://www.elastic.co/docs/reference/elasticsearch/configuration-reference/cluster-level-shard-allocation-routing-settings",
]
CORPUS = "url-evil-every-100-v1"


def value(row, indicators):
    return (f"https://evil{(row // 100) % indicators:04}.example/path" if row % 100 == 0
            else f"https://benign.example/path/{row}")


def checksum(pairs):
    """UTF-8 sorted lines: row_id TAB indicator_id TAB raw LF."""
    return hashlib.sha256("".join(f"{row}\t{indicator}\traw\n" for row, indicator in sorted(pairs)).encode()).hexdigest()


class Client:
    def __init__(self, state, timeout):
        self.base = state["url"].rstrip("/")
        self.timeout = timeout
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(self, method, path, body=None, ndjson=False):
        data = body.encode() if isinstance(body, str) else (json.dumps(body).encode() if body is not None else None)
        req = urllib.request.Request(self.base + path, data=data, method=method,
                                     headers={"Content-Type": "application/x-ndjson" if ndjson else "application/json"})
        try:
            with self.opener.open(req, timeout=self.timeout) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            # Response bodies contain useful engine errors; never include the URL.
            message = error.read().decode(errors="replace").replace(self.base, "[private endpoint]")
            raise RuntimeError(f"HTTP {error.code}: {message[:2000]}") from None


def record(out, data):
    with (out / "raw.jsonl").open("a") as stream:
        stream.write(json.dumps(data, separators=(",", ":")) + "\n")
    print(json.dumps({k: v for k, v in data.items() if k not in ("per_indicator", "stats", "probes")}), flush=True)


def mapping(field_type, metadata):
    return {"settings": {"number_of_shards": 1, "number_of_replicas": 0,
                         "refresh_interval": "-1", "requests.cache.enable": False,
                         "queries.cache.enabled": False},
            "mappings": {"dynamic": "strict", "_meta": metadata,
                         "properties": {"id": {"type": "long"}, "v": {"type": field_type}}}}


def setup(client, index, field_type, args, out):
    metadata = {"corpus": CORPUS, "rows": args.rows, "indicators": args.indicators,
                "owner": args.owner}
    try:
        found = client.request("GET", f"/{index}/_mapping")
    except RuntimeError as error:
        if not str(error).startswith("HTTP 404:"):
            raise
    else:
        if found[index]["mappings"].get("_meta") != metadata:
            raise RuntimeError("Existing index has a different corpus binding")
        count = client.request("GET", f"/{index}/_count")["count"]
        if count != args.rows:
            raise RuntimeError("Existing index is incomplete; use a fresh owned fixture")
        record(out, {"phase": "setup_reuse", "field_type": field_type, "rows": count})
        return
    client.request("PUT", f"/{index}", mapping(field_type, metadata))
    started = time.perf_counter()
    bulk_took = 0
    wire_bytes = 0
    source_bytes = 0
    corpus_hash = hashlib.sha256()
    for start in range(0, args.rows, args.bulk_size):
        lines = []
        for row in range(start, min(start + args.bulk_size, args.rows)):
            doc = {"id": row, "v": value(row, args.indicators)}
            source = json.dumps(doc, separators=(",", ":"))
            lines += [json.dumps({"create": {"_id": str(row)}}, separators=(",", ":")), source]
            source_bytes += len(source.encode())
            corpus_hash.update((source + "\n").encode())
        data = "\n".join(lines) + "\n"
        wire_bytes += len(data.encode())
        response = client.request("POST", f"/{index}/_bulk", data, ndjson=True)
        if response.get("errors"):
            raise RuntimeError("Bulk indexing error: " + json.dumps([v for v in response["items"] if v["create"].get("error")])[:1500])
        bulk_took += response.get("took", 0)
    client.request("POST", f"/{index}/_refresh")
    client.request("POST", f"/{index}/_flush")
    elapsed = time.perf_counter() - started
    stats = client.request("GET", f"/{index}/_stats/docs,store,indexing,segments,merge")
    if client.request("GET", f"/{index}/_count")["count"] != args.rows:
        raise RuntimeError("Indexed count differs from corpus")
    record(out, {"phase": "indexing", "field_type": field_type, "rows": args.rows,
                 "elapsed_seconds": elapsed, "bulk_server_took_sum_ms": bulk_took,
                 "source_json_bytes": source_bytes, "bulk_request_bytes": wire_bytes,
                 "corpus_sha256": corpus_hash.hexdigest(), "stats": stats["_all"],
                 "cost_scope": "client corpus generation+HTTP bulk+refresh+flush; no matching/watchlist ingestion",
                 "load_caveat": args.setup_load_note})


def query(indicator, query_type):
    literal = f"evil{indicator:04}.example"
    if query_type == "regexp":
        return {"regexp": {"v": {"value": ".*" + literal.replace(".", r"\.") + ".*", "flags": "NONE"}}}
    return {"wildcard": {"v": {"value": "*" + literal + "*", "case_insensitive": False}}}


def validate_response(response):
    if "error" in response or response.get("timed_out") or response.get("_shards", {}).get("failed", 0):
        raise RuntimeError("Search incomplete: " + json.dumps(response)[:1500])
    if response["hits"]["total"]["relation"] != "eq":
        raise RuntimeError("Search returned inexact count")


def paired_sweep(client, index, field_type, query_type, repeat, args, out):
    expected = [[] for _ in range(args.indicators)]
    for row in range(0, args.rows, 100):
        expected[(row // 100) % args.indicators].append(row)
    if max(map(len, expected)) > 10000:
        raise ValueError("Corpus has >10000 hits per indicator; reduce rows or increase indicators")
    details = []
    pairs = []
    response_bytes = 0
    started = time.perf_counter()
    for start in range(0, args.indicators, args.msearch_batch):
        lines = []
        indicators = range(start, min(start + args.msearch_batch, args.indicators))
        for indicator in indicators:
            lines += [json.dumps({"index": index, "request_cache": False}), json.dumps({
                "size": min(10000, max(1, len(expected[indicator]) + 1)), "track_total_hits": True,
                "_source": False, "query": query(indicator, query_type)})]
        result = client.request("POST", "/_msearch?max_concurrent_searches=4&max_concurrent_shard_requests=1",
                                "\n".join(lines) + "\n", ndjson=True)
        response_bytes += len(json.dumps(result, separators=(",", ":")).encode())
        responses = result["responses"]
        if len(responses) != len(indicators):
            raise RuntimeError("Missing msearch response")
        for indicator, response in zip(indicators, responses):
            validate_response(response)
            ids = sorted(int(hit["_id"]) for hit in response["hits"]["hits"])
            count = response["hits"]["total"]["value"]
            if ids != expected[indicator] or count != len(expected[indicator]):
                raise RuntimeError(f"Incorrect indicator {indicator}: count={count}, ids={ids[:12]}")
            pairs.extend((row, indicator) for row in ids)
            details.append({"indicator_id": indicator, "count": count, "ids": ids,
                            "server_took_ms": response["took"]})
    elapsed = time.perf_counter() - started
    record(out, {"phase": "paired_sweep", "field_type": field_type, "query_type": query_type,
                 "repeat": repeat, "elapsed_seconds": elapsed, "pair_count": len(pairs),
                 "pair_sha256": checksum(pairs), "per_indicator": details,
                 "server_took_sum_ms": sum(d["server_took_ms"] for d in details),
                 "server_took_median_ms": statistics.median(d["server_took_ms"] for d in details),
                 "server_took_max_ms": max(d["server_took_ms"] for d in details),
                 "decoded_response_json_bytes": response_bytes,
                 "clock_note": "server took sums overlap at concurrency4; not an elapsed duration",
                 "cache": "request/query caches disabled; filesystem/JIT warm state uncontrolled; repeats ordered"})
    return elapsed


def union_count(client, index, field_type, args, out):
    started = time.perf_counter()
    response = client.request("POST", f"/{index}/_search?request_cache=false", {
        "size": 0, "track_total_hits": True, "query": {"bool": {
            "should": [query(i, "wildcard") for i in range(args.indicators)], "minimum_should_match": 1}}})
    elapsed = time.perf_counter() - started
    validate_response(response)
    count = response["hits"]["total"]["value"]
    if count != len(range(0, args.rows, 100)):
        raise RuntimeError("Incorrect bool union count")
    record(out, {"phase": "bool_union_count", "field_type": field_type,
                 "elapsed_seconds": elapsed, "server_took_ms": response["took"], "distinct_row_count": count,
                 "scope": "distinct rows only; not indicator-row pairs, no hit materialization"})


def semantics(client, prefix, out):
    index = prefix + "-semantics"
    values = ["", "abc", "ABC", "a\nb", "a\\b", "é", "É", "k", "K", "K", "σ", "Σ", "ς", "ß", "ss", "a\0b"]
    try:
        client.request("PUT", f"/{index}", mapping("wildcard", {"probe": "v1"}))
    except RuntimeError as error:
        if "resource_already_exists_exception" not in str(error):
            raise
    lines = []
    for row, text in enumerate(values):
        lines += [json.dumps({"index": {"_id": str(row)}}), json.dumps({"id": row, "v": text})]
    client.request("POST", f"/{index}/_bulk?refresh=true", "\n".join(lines) + "\n", ndjson=True)
    cases = [("empty_pattern", "", False, []), ("all_including_empty", "*", False, list(range(len(values)))),
             ("newline", "*a?b*", False, [3, 4, 15]), ("backslash", r"*\\*", False, [4]),
             ("ascii_casefold", "*abc*", True, [1, 2]), ("unicode_accent_casefold", "*é*", True, None),
             ("unicode_kelvin_casefold", "*k*", True, None), ("unicode_sigma_casefold", "*σ*", True, None),
             ("unicode_multichar_casefold", "*ss*", True, None)]
    probes = []
    for name, pattern, insensitive, expected in cases:
        response = client.request("POST", f"/{index}/_search", {"size": 100, "track_total_hits": True,
            "query": {"wildcard": {"v": {"value": pattern, "case_insensitive": insensitive}}}})
        validate_response(response)
        ids = sorted(int(hit["_id"]) for hit in response["hits"]["hits"])
        if expected is not None and ids != expected:
            raise RuntimeError(f"Semantic probe {name}: {ids} != {expected}")
        probes.append({"name": name, "pattern": pattern, "case_insensitive": insensitive,
                       "matched_ids": ids, "matched_values": [values[i] for i in ids],
                       "expected_ids": expected})
    record(out, {"phase": "semantic_probe", "values": values, "probes": probes,
                 "limitation": "JSON Unicode strings cannot represent arbitrary invalid UTF-8 byte values; empty Elastic wildcard expression matches no values (unlike an empty unanchored RE2 regex); Unicode casefold observations are not a claim of RE2 parity"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--rows", type=int, default=1_000_000)
    parser.add_argument("--indicators", type=int, default=1000)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--outresults", type=Path, required=True)
    parser.add_argument("--phase", choices=["setup", "queries", "all"], default="all")
    parser.add_argument("--fields", default="wildcard,keyword")
    parser.add_argument("--query-types", default="wildcard")
    parser.add_argument("--bulk-size", type=int, default=5000)
    parser.add_argument("--msearch-batch", type=int, default=100)
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument("--slow-repeat-seconds", type=float, default=300)
    parser.add_argument("--union", action="store_true")
    parser.add_argument("--setup-load-note", default="No exclusive CPU reservation for index setup; other builds may be active")
    args = parser.parse_args()
    if not 0 < args.rows <= 10_000_000 or not 0 < args.indicators <= 10_000 or not 0 < args.repeats <= 10:
        parser.error("rows/indicators/repeats outside bounded benchmark range")
    if not 0 < args.bulk_size <= 10000 or not 0 < args.msearch_batch <= 100:
        parser.error("bulk/msearch batch outside bounded range")
    fields = args.fields.split(",")
    kinds = args.query_types.split(",")
    if not set(fields) <= {"wildcard", "keyword"} or not set(kinds) <= {"wildcard", "regexp"}:
        parser.error("Unsupported field/query type")
    state = json.loads(args.state.read_text())
    if state.get("engine") != "elasticsearch" or not state.get("owner"):
        parser.error("Requires owned elasticsearch fixture state")
    info = json.loads(subprocess.check_output(["docker", "inspect", state["name"]], text=True))[0]
    if info["Config"]["Labels"].get("wildcard.benchmark.owner") != state["owner"]:
        parser.error("Fixture ownership mismatch")
    args.owner = state["owner"]
    prefix = f"wc-bench-{args.owner}-{args.rows}-{args.indicators}"
    out = args.outresults
    out.mkdir(parents=True, exist_ok=True)
    client = Client(state, args.timeout)
    version = client.request("GET", "/")["version"]
    node_stats = next(iter(client.request("GET", "/_nodes/stats/jvm")["nodes"].values()))
    cpu_model = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                      if line.startswith("model name")), "unknown")
    settings = {"engine": "Elasticsearch", "version": version, "image": state["image"],
                "cpus": state["cpus"], "memory_bytes": state["memory_bytes"],
                "cluster_overrides": client.request("GET", "/_cluster/settings"),
                "jvm_heap_max_bytes": node_stats["jvm"]["mem"]["heap_max_in_bytes"],
                "machine": {"platform": platform.platform(), "architecture": platform.machine(), "cpu_model": cpu_model,
                            "host_logical_cpus": os.cpu_count()},
                "corpus": CORPUS, "rows": args.rows, "indicators": args.indicators,
                "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                "host_loadavg_at_start": os.getloadavg(),
                "mapping": {field: mapping(field, {}) for field in fields}, "repeats_requested": args.repeats,
                "transport": "native Elasticsearch HTTP JSON; differs from Antalya Arrow/PG FDW",
                "pair_checksum": "SHA256 sorted UTF-8 lines row_id TAB indicator_id TAB raw LF",
                "msearch_batch": args.msearch_batch, "max_concurrent_searches": 4,
                "query_types": kinds, "docs": DOCS, "invalid_utf8": "not representable in JSON string corpus",
                "cache_policy": "query and request caches disabled; no OS cache drop; no artificial cold-cache claim"}
    settings_bytes = json.dumps(settings, indent=2) + "\n"
    (out / "settings.json").write_text(settings_bytes)
    (out / f"settings-{args.phase}.json").write_text(settings_bytes)
    if args.phase in ("setup", "all"):
        semantics(client, prefix, out)
        for field in fields:
            setup(client, prefix + "-" + field, field, args, out)
    if args.phase in ("queries", "all"):
        for field in fields:
            index = prefix + "-" + field
            found = client.request("GET", f"/{index}/_mapping")[index]["mappings"]
            if found.get("_meta") != {"corpus": CORPUS, "rows": args.rows,
                                      "indicators": args.indicators, "owner": args.owner}:
                raise RuntimeError("Corpus binding mismatch")
            for kind in kinds:
                for repeat in range(args.repeats):
                    try:
                        duration = paired_sweep(client, index, field, kind, repeat, args, out)
                    except Exception as error:
                        record(out, {"phase": "query_error", "field_type": field, "query_type": kind,
                                     "repeat": repeat, "error": str(error).replace(client.base, "[private endpoint]")})
                        raise
                    if duration > args.slow_repeat_seconds:
                        record(out, {"phase": "repeat_cap", "field_type": field, "query_type": kind,
                                     "reason": "previous repeat exceeded slow-repeat budget", "seconds": duration})
                        break
            if args.union:
                union_count(client, index, field, args, out)


if __name__ == "__main__":
    main()
