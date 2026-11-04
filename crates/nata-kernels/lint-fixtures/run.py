#!/usr/bin/env python3
"""Proves the strictness lints of ADR 0003 (decision 6) can fail.

Each file under ban/, lints/ and controls/ is compiled as the lib of a scratch crate that has the SAME
[lints] table and the SAME clippy.toml as crates/nata-kernels (both are copied from there at run time),
then `cargo clippy` runs on it. The first line of a fixture says what must happen:
    // expect: <clippy::lint_name> [text that must appear in the message]
    // expect: pass            (clippy must exit 0 with no warnings: a control)
A fixture passes the check only if clippy fails WITH the named lint (a compile error or another lint does
not count). Then the real kernel crate must pass clippy with -D warnings. Exit status is non-zero on any
mismatch. Needs the arrow crates (downloaded on first use; set NATA_OFFLINE=1 to forbid network access and use the registry cache only).
Usage: run.py [fixture-name ...]   (from anywhere)
"""
import json, os, re, shutil, subprocess, sys, tempfile

def _pinned_toolchain():
    """The fixtures run cargo in scratch directories OUTSIDE the repository, where rustup does not see the repo's
    rust-toolchain.toml; on a machine without a default toolchain that fails ('no default is configured'). Pin the
    repo's channel explicitly. Found by the first clean-machine run on GCP."""
    if os.environ.get("RUSTUP_TOOLCHAIN"):
        return os.environ["RUSTUP_TOOLCHAIN"]
    here = os.path.dirname(os.path.abspath(__file__))
    d = here
    for _ in range(6):
        f = os.path.join(d, "rust-toolchain.toml")
        if os.path.exists(f):
            m = re.search(r'channel\s*=\s*"([^"]+)"', open(f).read())
            if m:
                return m.group(1)
        d = os.path.dirname(d)
    return None

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
ROOT = os.path.abspath(os.path.join(CRATE, "..", ".."))
TARGET = os.path.join(ROOT, "target", "lint-fixtures")
# Offline only when asked for (NATA_OFFLINE=1): a fresh machine has not downloaded the crates the fixtures need,
# and `--offline` then fails to resolve them (found by the second clean-machine run on GCP).
OFFLINE = ["--offline"] if os.environ.get("NATA_OFFLINE") else []


def lints_table():
    text = open(os.path.join(CRATE, "Cargo.toml")).read()
    m = re.search(r"^\[lints\.rust\].*", text, re.S | re.M)
    return m.group(0)


def scratch(workdir, src, with_config=True):
    os.makedirs(os.path.join(workdir, "src"), exist_ok=True)
    if with_config:
        shutil.copy(os.path.join(CRATE, "clippy.toml"), os.path.join(workdir, "clippy.toml"))
    shutil.copy(os.path.join(ROOT, "Cargo.lock"), os.path.join(workdir, "Cargo.lock"))
    shutil.copy(src, os.path.join(workdir, "src", "lib.rs"))
    deps = "\n".join(f'{c} = "60"' for c in ["arrow-arith", "arrow-array", "arrow-buffer", "arrow-cast", "arrow-schema", "arrow-select"])
    open(os.path.join(workdir, "Cargo.toml"), "w").write(
        f'[package]\nname = "fixture"\nversion = "0.0.0"\nedition = "2024"\npublish = false\n\n'
        f'[dependencies]\n{deps}\narrow = {{ version = "60", default-features = false }}\n\n'
        f"{lints_table()}\n[workspace]\n"
    )


def clippy(workdir):
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET)
    if _pinned_toolchain(): env['RUSTUP_TOOLCHAIN'] = _pinned_toolchain()
    r = subprocess.run(["cargo", "clippy", *OFFLINE, "--message-format=json", "--", "-A", "unused"], cwd=workdir, capture_output=True, text=True, env=env)
    diags = []
    for line in r.stdout.splitlines():
        try:
            m = json.loads(line)
        except ValueError:
            continue
        if m.get("reason") == "compiler-message" and m["message"]["level"] in ("error", "warning"):
            d = m["message"]
            if d["message"].startswith("aborting due to") or "warning emitted" in d["message"] or "warnings emitted" in d["message"]:
                continue
            diags.append(((d.get("code") or {}).get("code"), d["level"], d["message"]))
    return r.returncode, diags, r.stderr


def main():
    only = set(sys.argv[1:])
    results, bad = [], 0
    fixtures = sorted(f"{d}/{f}" for d in ("ban", "lints", "controls", "gaps") for f in os.listdir(os.path.join(HERE, d)) if f.endswith(".rs"))
    for rel in fixtures:
        name = rel[:-3]
        if only and name.split("/")[1] not in only:
            continue
        src = os.path.join(HERE, rel)
        first = open(src).readline().strip()
        m = re.match(r"// expect: (\S+)(?: (.*))?$", first)
        assert m, f"{rel}: first line must be '// expect: ...'"
        want, needle = m.group(1), m.group(2)
        with tempfile.TemporaryDirectory(prefix="nata-fixture-") as wd:
            scratch(wd, src)
            rc, diags, err = clippy(wd)
        if want == "pass":
            ok = rc == 0 and not diags
            detail = "clean" if ok else f"rc={rc} diags={diags[:3]} {err.strip()[-200:]}"
        else:
            hits = [d for d in diags if d[0] == want and (not needle or needle in d[2])]
            ok = rc != 0 and bool(hits)
            detail = f"{want} x{len(hits)}" if ok else f"rc={rc} wanted {want}{' / ' + needle if needle else ''}; got {[(d[0], d[2][:60]) for d in diags][:4]} {err.strip()[-200:] if not diags else ''}"
        bad += not ok
        results.append((name, ok, detail))
        print(f"{'ok  ' if ok else 'FAIL'} {name:<40} {detail}", flush=True)
    if not only:
        # the bans must come from clippy.toml: without it the same fixture compiles clean
        with tempfile.TemporaryDirectory(prefix="nata-fixture-") as wd:
            scratch(wd, os.path.join(HERE, "ban", "add_wrapping.rs"), with_config=False)
            rc, diags, err = clippy(wd)
        ok = rc == 0 and not any(d[0] == "clippy::disallowed_methods" for d in diags)
        bad += not ok
        print(f"{'ok  ' if ok else 'FAIL'} {'sanity: ban/add_wrapping WITHOUT clippy.toml is not flagged':<40} {'clean' if ok else diags}")
        r = subprocess.run(["cargo", "clippy", "-p", "nata-kernels", "--all-targets", *OFFLINE, "--", "-D", "warnings"], cwd=ROOT, capture_output=True, text=True)
        ok = r.returncode == 0 and "warning" not in r.stderr.replace("generated", "")
        bad += not ok
        print(f"{'ok  ' if ok else 'FAIL'} {'real crate: cargo clippy -p nata-kernels --all-targets -D warnings':<40} {'clean' if ok else r.stderr[-300:]}")
    expect_fail = [r for r in results if not r[0].startswith(("controls/", "gaps/"))]
    clean = [r for r in results if r[0].startswith("controls/")]
    gaps = [r for r in results if r[0].startswith("gaps/")]
    print(f"\n{sum(1 for r in expect_fail if r[1])} of {len(expect_fail)} fixtures fail with the expected lint; "
          f"{sum(1 for r in clean if r[1])} of {len(clean)} controls are clean; "
          f"{sum(1 for r in gaps if r[1])} of {len(gaps)} known gaps still pass clippy (a gap that starts failing means the lint improved)")
    sys.exit(1 if bad else 0)


main()
