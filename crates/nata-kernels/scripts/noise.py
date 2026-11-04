#!/usr/bin/env python3
"""How noisy are the strictness lints on real kernel code? (ADR 0003 decision 6)

Takes spikes/generic-ops/src/kernels.rs (the kernel code as it was, with no lint resolution), puts it in a scratch
crate with the lint table and clippy.toml of crates/nata-kernels but the three lints set to `warn`, and counts every
diagnostic, split into code before and inside `#[cfg(test)]`. Then counts what is left in crates/nata-kernels/src/kernels.rs
(zero diagnostics) and how many `#[allow(...)]` attributes it took to get there.
Usage: noise.py   (from anywhere)
"""
import collections, json, os, re, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
ROOT = os.path.abspath(os.path.join(CRATE, "..", ".."))
SPIKE = os.path.join(ROOT, "spikes", "generic-ops", "src", "kernels.rs")
LINTS = ["clippy::arithmetic_side_effects", "clippy::cast_possible_truncation", "clippy::cast_possible_wrap"]


def count(src_text):
    with tempfile.TemporaryDirectory(prefix="nata-noise-") as wd:
        os.makedirs(os.path.join(wd, "src"))
        shutil.copy(os.path.join(CRATE, "clippy.toml"), wd)
        shutil.copy(os.path.join(ROOT, "Cargo.lock"), wd)
        open(os.path.join(wd, "src", "lib.rs"), "w").write(src_text)
        open(os.path.join(wd, "Cargo.toml"), "w").write(
            '[package]\nname = "noise"\nversion = "0.0.0"\nedition = "2024"\n[dependencies]\narrow-buffer = "60"\n[workspace]\n')
        env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(ROOT, "target", "lint-fixtures"))
        flags = [x for l in LINTS for x in ("-W", l)]
        r = subprocess.run(["cargo", "clippy", "--offline", "--tests", "--message-format=json", "--"] + flags, cwd=wd, capture_output=True, text=True, env=env)
    test_line = next(i for i, l in enumerate(src_text.splitlines(), 1) if l.strip() == "#[cfg(test)]")
    seen, c = set(), collections.Counter()
    for line in r.stdout.splitlines():
        try:
            m = json.loads(line)
        except ValueError:
            continue
        if m.get("reason") != "compiler-message":
            continue
        d = m["message"]
        code = (d.get("code") or {}).get("code")
        sp = [s for s in d["spans"] if s["is_primary"]]
        if code in LINTS and sp:
            key = (code, sp[0]["line_start"], sp[0]["column_start"])  # lib and lib-test targets both report non-test code
            if key not in seen:
                seen.add(key)
                c[(code.split("::")[1], "inside #[cfg(test)]" if sp[0]["line_start"] >= test_line else "library code")] += 1
    return c


def main():
    old = open(SPIKE).read().replace("use arrow::buffer::NullBuffer;", "use arrow_buffer::NullBuffer;")
    print("spikes/generic-ops/src/kernels.rs as written (no resolution), lints at warn:")
    c = count(old)
    for (lint, where), n in sorted(c.items(), key=lambda x: (x[0][1], x[0][0])):
        print(f"  {n:3}  {lint:<26} {where}")
    print(f"  total: library code {sum(n for (l, w), n in c.items() if w == 'library code')}, tests {sum(n for (l, w), n in c.items() if w != 'library code')}")
    new = open(os.path.join(CRATE, "src", "kernels.rs")).read()
    c2 = count(new)
    print(f"\ncrates/nata-kernels/src/kernels.rs now (includes the new div kernels), lints at warn: {sum(c2.values())} diagnostics")
    body, tests = new.split("#[cfg(test)]", 1)
    allows = re.findall(r"#\[allow\(([^\]]*?)\)\]", body, re.S)
    print(f"#[allow] attributes in library code: {len(allows)}")
    for a in allows:
        print("   ", " ".join(a.split()))
    print(f"#[allow] attributes on the test module: {len(re.findall(r'#\[allow\(', tests))}")


main()
