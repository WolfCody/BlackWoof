#!/usr/bin/env python3
"""Strict check of the compile-time-disabled `log` facade.

Reads `cargo metadata --format-version 1` JSON from stdin and exits non-zero unless:
  * the resolved graph contains exactly one version of the `log` crate, and
  * that crate is built with the `max_level_off` feature (every log call is compiled out).

`log` cannot be removed (every Symphonia crate depends on it). Real logging stays forbidden
elsewhere: the dependency denylist, the absence of Android logging symbols in the library
(scripts/verify_apk.sh) and the absence of logger initialisation in the sources.

Usage:
  cargo metadata --format-version 1 --filter-platform aarch64-linux-android --locked \
    | python3 scripts/check_log_facade.py
"""
import json
import sys


def main() -> int:
    meta = json.load(sys.stdin)
    resolve = meta.get("resolve")
    if not resolve:
        print("FAIL: cargo metadata returned no resolved dependency graph")
        return 1

    # Map package id -> (name, version); id formats differ between cargo versions.
    info = {p["id"]: (p["name"], p["version"]) for p in meta.get("packages", [])}
    nodes = [n for n in resolve.get("nodes", []) if info.get(n["id"], ("", ""))[0] == "log"]

    for node in nodes:
        version = info[node["id"]][1]
        print("log %s: features = %s" % (version, sorted(node.get("features", []))))

    if not nodes:
        print("FAIL: the log crate is not in the resolved graph")
        return 1
    if len(nodes) > 1:
        versions = ", ".join(sorted(info[n["id"]][1] for n in nodes))
        print("FAIL: several versions of the log crate are present: " + versions)
        return 1
    if "max_level_off" not in nodes[0].get("features", []):
        print("FAIL: the log crate is built without the max_level_off feature")
        return 1

    print("OK: exactly one log crate, built with max_level_off (all log calls are compiled out)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
