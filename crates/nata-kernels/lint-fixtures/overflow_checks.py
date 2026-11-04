#!/usr/bin/env python3
"""Proves `overflow-checks = true` turns silent integer wrap-around into a panic in a RELEASE build, and that the
per-package profile entry reaches code that other crates call. Builds scratch crates (offline, no dependencies).
Usage: overflow_checks.py   (from anywhere; exit status non-zero if an expectation fails)
"""
import os, subprocess, sys, tempfile, textwrap

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
TARGET = os.path.join(ROOT, "target", "lint-fixtures-overflow")
ENV = dict(os.environ, CARGO_TARGET_DIR=TARGET)

# op: (with overflow-checks, without)
EXPECT = {
    "add_i32": ("panic add", "value -2147483648"),
    "sub_i32": ("panic subtract", "value 2147483647"),
    "mul_i32": ("panic multiply", "value -2"),
    "add_i64": ("panic add", "value -9223372036854775808"),
    "neg_i32_min": ("panic negate", "value -2147483648"),
    "abs_i32_min": ("panic", "value -2147483648"),
    "pow_i32": ("panic exponentiate", "value 1"),
    "shl_amount": ("panic shift left", "value 256"),
    "shl_bits_lost": ("value -9223372036854775808", "value -9223372036854775808"),  # GAP: neither mode notices
    "div_min_neg1": ("panic divide", "panic divide"),  # always, in every profile
    "div_zero": ("panic divide by zero", "panic divide by zero"),
    "truncating_cast": ("value 0", "value 0"),  # `as` never panics: only cast_possible_truncation sees it
}


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w").write(textwrap.dedent(text).lstrip())


def run(cmd, cwd):
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, env=ENV)


def outcome(r):
    if r.returncode == 0:
        return "value " + r.stdout.strip()
    msg = [l for l in r.stderr.splitlines() if "panicked" in l or "attempt to" in l]
    text = " ".join(msg)
    if "with overflow" in text:
        verb = text.split("attempt to ")[1].split(" with overflow")[0]
        return "panic " + verb
    if "divide by zero" in text:
        return "panic divide by zero"
    if "attempt to divide" in text:
        return "panic divide"
    return "panic " + text[-80:]


def main():
    bad = 0
    with tempfile.TemporaryDirectory(prefix="nata-overflow-") as wd:
        for setting in (True, False):
            d = os.path.join(wd, f"probe-{setting}")
            write(os.path.join(d, "Cargo.toml"), f"""
                [package]
                name = "probe"
                version = "0.0.0"
                edition = "2024"
                [profile.release]
                overflow-checks = {str(setting).lower()}
                [workspace]
                """)
            os.makedirs(os.path.join(d, "src"))
            open(os.path.join(d, "src", "main.rs"), "w").write(open(os.path.join(HERE, "overflow", "probe.rs")).read())
            b = run(["cargo", "build", "--release", "--offline", "-q"], d)
            assert b.returncode == 0, b.stderr
            for op, expect in EXPECT.items():
                got = outcome(run([os.path.join(TARGET, "release", "probe"), op], d))
                want = expect[0] if setting else expect[1]
                ok = got.startswith(want) if want.startswith("panic") else got == want
                bad += not ok
                print(f"{'ok  ' if ok else 'FAIL'} overflow-checks={str(setting).lower():<5} {op:<16} -> {got}" + ("" if ok else f"   (wanted {want})"))

        # Where must the setting live? Two crates: k (the kernels) and app (the caller, no setting of its own).
        for label, profile, want in [
            ("per-package entry on k only", "[profile.release.package.k]\noverflow-checks = true",
             {"add_never": "panic add", "add_plain": "value -2147483648", "add_inline": "value -2147483648", "add_generic": "value -2147483648", "own_add": "value -2147483648"}),
            ("workspace-wide [profile.release]", "[profile.release]\noverflow-checks = true",
             {"add_never": "panic add", "add_plain": "panic add", "add_inline": "panic add", "add_generic": "panic add", "own_add": "panic add"}),
        ]:
            d = os.path.join(wd, "ws-" + label.split()[0])
            write(os.path.join(d, "Cargo.toml"), '[workspace]\nresolver = "3"\nmembers = ["k", "app"]\n' + profile + "\n")
            write(os.path.join(d, "k", "Cargo.toml"), '[package]\nname = "k"\nversion = "0.0.0"\nedition = "2024"\n')
            write(os.path.join(d, "k", "src", "lib.rs"), """
                #[inline(never)]
                pub fn add_never(a: i32, b: i32) -> i32 { a + b }
                pub fn add_plain(a: i32, b: i32) -> i32 { a + b }
                #[inline(always)]
                pub fn add_inline(a: i32, b: i32) -> i32 { a + b }
                pub fn add_generic<T: std::ops::Add<Output = T>>(a: T, b: T) -> T { a + b }
                """)
            write(os.path.join(d, "app", "Cargo.toml"), '[package]\nname = "app"\nversion = "0.0.0"\nedition = "2024"\n[dependencies]\nk = { path = "../k" }\n')
            write(os.path.join(d, "app", "src", "main.rs"), """
                use std::hint::black_box;
                fn main() {
                    let op = std::env::args().nth(1).unwrap_or_default();
                    let (a, b) = (black_box(i32::MAX), black_box(1));
                    println!("{}", match op.as_str() {
                        "add_never" => k::add_never(a, b),
                        "add_plain" => k::add_plain(a, b),
                        "add_inline" => k::add_inline(a, b),
                        "add_generic" => k::add_generic(a, b),
                        "own_add" => a + b,
                        _ => panic!("unknown"),
                    });
                }
                """)
            b = run(["cargo", "build", "--release", "--offline", "-q"], d)
            assert b.returncode == 0, b.stderr
            exe = os.path.join(TARGET, "release", "app")
            for op, w in want.items():
                got = outcome(run([exe, op], d))
                ok = got.startswith(w) if w.startswith("panic") else got == w
                bad += not ok
                print(f"{'ok  ' if ok else 'FAIL'} {label:<34} app unchecked, call {op:<12} -> {got}" + ("" if ok else f"   (wanted {w})"))
    sys.exit(1 if bad else 0)


main()
