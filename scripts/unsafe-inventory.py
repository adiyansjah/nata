#!/usr/bin/env python3
"""A textual inventory of `unsafe` in the dependency tree of a workspace package: the stand-in for `cargo geiger` when geiger cannot be installed
(it is not, offline). For every package reachable from PACKAGE through normal and build dependencies (dev-dependencies excluded), it reads the
sources of the package's own target(s) and counts the `unsafe` keywords in code: blocks, functions, impls and traits, after removing comments, string
and character literals. It also reports whether the crate root declares `#![forbid(unsafe_code)]` or `#![deny(unsafe_code)]`.
It does NOT expand macros, does NOT see through `cfg`, counts unsafe code behind features that are off, and counts `unsafe` in tests inside src; it is
an upper bound on the code a build can contain and an approximation of what geiger reports, not geiger's own numbers.
usage: scripts/unsafe-inventory.py [PACKAGE]   (default nata-parquet; run from the repository root; uses `cargo metadata --offline` for Linux x86-64; NATA_TARGET=aarch64-unknown-linux-gnu for ARM64)"""
import json, os, re, subprocess, sys

def strip(src):
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            i = n if j < 0 else j
        elif src.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if src.startswith("/*", i): depth, i = depth + 1, i + 2
                elif src.startswith("*/", i): depth, i = depth - 1, i + 2
                else: i += 1
        elif c == '"' or (c == 'r' and re.match(r'r#*"', src[i:i + 12])) or (c == 'b' and src.startswith('b"', i)):
            m = re.match(r'(b?)(r(#*))?"', src[i:])
            if m.group(2):
                end = '"' + m.group(3)
                j = src.find(end, i + m.end())
                i = n if j < 0 else j + len(end)
            else:
                i += m.end()
                while i < n and src[i] != '"':
                    i += 2 if src[i] == "\\" else 1
                i += 1
            out.append('""')
        elif c == "'":
            m = re.match(r"'(\\.[^']*|[^\\'])'", src[i:])
            if m: i += m.end(); out.append("' '")
            else: out.append(c); i += 1
        else:
            out.append(c); i += 1
    return "".join(out)

def main():
    pkg = sys.argv[1] if len(sys.argv) > 1 else "nata-parquet"
    md = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--offline", "--locked", "--filter-platform", os.environ.get("NATA_TARGET", "x86_64-unknown-linux-gnu")], text=True))
    byid = {p["id"]: p for p in md["packages"]}
    nodes = {n["id"]: n for n in md["resolve"]["nodes"]}
    root = next(p["id"] for p in md["packages"] if p["name"] == pkg and p["source"] is None)
    seen, stack = set(), [root]
    while stack:
        x = stack.pop()
        if x in seen: continue
        seen.add(x)
        for d in nodes[x]["deps"]:
            if any(k["kind"] in (None, "build") for k in d["dep_kinds"]):
                stack.append(d["pkg"])
    rows = []
    for pid in seen:
        p = byid[pid]
        root_dir = os.path.dirname(p["manifest_path"])
        total, files = 0, 0
        declared = ""
        for t in p["targets"]:
            if "test" in t["kind"] or "bench" in t["kind"] or "example" in t["kind"]:
                continue
            base = os.path.dirname(t["src_path"])
            for d, _, fs in os.walk(base):
                for f in fs:
                    if f.endswith(".rs"):
                        text = open(os.path.join(d, f), errors="replace").read()
                        code = strip(text)
                        total += len(re.findall(r"\bunsafe\b", code))
                        files += 1
                        if os.path.join(d, f) == t["src_path"] and re.search(r"#!\[(forbid|deny)\(\s*unsafe_code", text):
                            declared = "forbid/deny"
        rows.append((p["name"], p["version"], "workspace" if p["source"] is None else "registry", total, files, declared))
    rows.sort(key=lambda r: (-r[3], r[0]))
    print(f"package {pkg}: {len(rows)} packages in its normal+build dependency tree (cargo metadata, this machine's feature resolution)")
    print(f"{'package':<28}{'version':<12}{'source':<10}{'unsafe':>8}{'files':>7}  root declares")
    for r in rows:
        print(f"{r[0]:<28}{r[1]:<12}{r[2]:<10}{r[3]:>8}{r[4]:>7}  {r[5]}")
    print(f"total `unsafe` keywords in code: {sum(r[3] for r in rows)} in {sum(1 for r in rows if r[3])} of {len(rows)} packages; "
          f"workspace packages: {sum(r[3] for r in rows if r[2] == 'workspace')}")

main()
