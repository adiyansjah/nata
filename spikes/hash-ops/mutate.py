#!/usr/bin/env python3
"""Mutation check of the hash-ops tests.

Applies ONE deliberate breakage at a time to a PRIVATE COPY of the spike (target/mutate/work), never to the
real sources, and runs the checks that are supposed to catch it, cheapest first:
  tests      cargo test --release            (property tests against the scalar reference + kernel tests)
  nullcheck  hash-ops nullcheck <data>       (10/50/100% NULLs on every column read, engine vs scalar reference)
  duckdb     check.sh <data>                 (7 queries, 1/4/8 threads, digit for digit against DuckDB)
A mutant is CAUGHT by the first layer that fails; SURVIVED if every layer passes; INVALID if it does not
compile (a bug in the mutant, not evidence). Exit status is 1 if anything survived or was invalid.

Usage: mutate.py [--list] [--matrix] [--layer tests|nullcheck|duckdb] [--dry] [name ...]
  --list     print one exact mutant name per line and exit (nothing else is done)
  name       a bare argument selects the mutant with exactly that name; if there is none, every mutant whose
             name contains it (several names may be given; no names = all mutants)
  --matrix   run every layer for every mutant and show which layers caught it (slower)
  --layer L  run only layer L
  --dry      only check that every edit applies exactly once
Data: $HASH_OPS_DATA or <repo>/data/tpch-sf1.
Private work: $HASH_OPS_MUT_ROOT (default <this dir>/target/mutate) holds the copy being mutated (work/) and its
cargo target dir (target/). Give each concurrent copy or shard its own value, or they will overwrite each other.
Nothing else in the run uses a fixed path (check.sh writes to its own mktemp dir; cargo gets CARGO_TARGET_DIR).
"""
import os, shutil, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
DATA = os.environ.get("HASH_OPS_DATA", os.path.join(ROOT, "data", "tpch-sf1"))
MUT = os.path.abspath(os.environ.get("HASH_OPS_MUT_ROOT") or os.path.join(HERE, "target", "mutate"))
WORK = os.path.join(MUT, "work")
TGT = os.path.join(MUT, "target")
BIN = os.path.join(TGT, "release", "hash-ops")
COPY = ["Cargo.toml", "Cargo.lock", "src", "tests", "sql", "check.sh"]
TIMEOUT = int(os.environ.get("MUTATE_TIMEOUT", "1500"))

# (name, [(file, old, new), ...]) -- every `old` must occur exactly once in its file.
M = []
def mut(name, *edits):
    M.append((name, list(edits)))

mut("first group has a NULL key: NULL flag lost (the original bug)", ("src/keys.rs",
"""            if null {
                if sc.nulls.is_empty() {
                    sc.nulls.resize(len, 0);
                }
                sc.nulls.push(1);
            } else if !sc.nulls.is_empty() {
                sc.nulls.push(0);
            }""",
"""            if null && sc.nulls.is_empty() {
                sc.nulls.resize(len, 0);
            }
            if !sc.nulls.is_empty() {
                sc.nulls.push(null as u8);
            }"""))
mut("string filter looks up the dictionary before the NULL test (original bug)", ("src/engine.rs",
"|i| !kc.is_null(i) && ok[keys[i] as usize])", "|i| ok[keys[i] as usize] & !kc.is_null(i))"))
mut("string flag looks up the dictionary before the NULL test", ("src/engine.rs",
"(!kc.is_null(i) && (ok[keys[i] as usize] != *negate))", "(!kc.is_null(i) & (ok[keys[i] as usize] != *negate))"))
mut("join build keeps rows with a NULL key", ("src/join.rs",
"|i| !kcols.iter().any(|c| c.is_null(i)));", "|i| !kcols.iter().any(|c| c.is_null(i)) | true);"))
mut("join build keeps NULL-key rows when there is no selection", ("src/join.rs",
"let identity = b.sel.is_none() && keep.len() == b.rows;", "let identity = b.sel.is_none();"))
mut("join probe does not skip rows with a NULL key", ("src/join.rs",
"if has_null && pcols.iter().any(|c| c.is_null(row)) {", "if has_null && pcols.iter().any(|c| c.is_null(row)) && false {"))
mut("fan-out: only the first match of a chain is emitted", ("src/join.rs",
"l = part.next[l as usize];", "l = END;"))
mut("fan-out: the build chain drops earlier duplicates", ("src/join.rs",
"next[l] = *hd;", "next[l] = END;"))
mut("fan-out: every match emitted twice", ("src/join.rs",
"""                pr.push(row as u32);
                br.push(part.rows[l as usize]);""",
"""                pr.push(row as u32);
                br.push(part.rows[l as usize]);
                pr.push(row as u32);
                br.push(part.rows[l as usize]);"""))
mut("probe ignores the selection vector", ("src/join.rs",
"let n = b.live();\n        if n == 0 || table.n == 0 {", "let n = b.rows;\n        if n == 0 || table.n == 0 {"),
("src/join.rs", "let (rows, sel) = (b.rows, b.sel.as_deref());\n        let pcols", "let (rows, sel) = (b.rows, None::<&[u32]>);\n        let pcols"))
mut("build ignores the selection vector", ("src/join.rs",
"let keep = filter_with(b.rows, b.sel.as_deref(),", "let keep = filter_with(b.rows, None,"))
mut("aggregate ignores the selection vector", ("src/agg.rs",
"let n = b.live();\n        if n == 0 {\n            return Ok(());\n        }\n        let (rows, sel) = (b.rows, b.sel.as_deref());",
"let n = b.rows;\n        if n == 0 {\n            return Ok(());\n        }\n        let (rows, sel) = (b.rows, None::<&[u32]>);"))
mut("merge loses the last thread's partial table", ("src/agg.rs",
"for (t, l) in locals.iter().enumerate() {", "for (t, l) in locals.iter().enumerate().take(locals.len() - 1) {"))
mut("merge drops the COUNT(*) of the partial tables", ("src/agg.rs",
"for j in 0..s {\n                self.state[g * s + j] += src.state[sg * s + j];", "for j in 1..s {\n                self.state[g * s + j] += src.state[sg * s + j];"))
mut("merge drops the NULL-skip counters of the partial tables", ("src/agg.rs",
"self.nulls[g * k + j] += src.nulls[sg * k + j];", "self.nulls[g * k + j] += 0;"))
mut("NULL group dropped from the output", ("src/agg.rs",
"        for g in 0..self.groups {\n            keys.clear();",
"        for g in 0..self.groups {\n            if store.cols.iter().any(|c| !c.nulls.is_empty() && c.nulls[g] != 0) {\n                continue;\n            }\n            keys.clear();"))
mut("hash collision treated as equality (aggregate)", ("src/agg.rs",
"self.table.find(h, |&g| store.eq(g as usize, &cols, row))", "self.table.find(h, |_| true)"))
mut("hash collision treated as equality (aggregate merge)", ("src/agg.rs",
"self.table.find(h, |&g| store.eq(g as usize, &scols, sg))", "self.table.find(h, |_| true)"))
mut("hash collision treated as equality (join probe)", ("src/join.rs",
"bcols.iter().zip(&pcols).all(|(bc, pc)| bc.eq_at(brow, pc, row))", "bcols.iter().zip(&pcols).all(|(bc, pc)| bc.eq_at(brow, pc, row) || true)"))
mut("hash collision treated as equality (join build)", ("src/join.rs",
"kc.iter().all(|c| c.eq_at(rows[hd as usize] as usize, c, g))", "kc.iter().all(|c| c.eq_at(rows[hd as usize] as usize, c, g) || true)"))
mut("group-key equality ignores NULL against a value", ("src/keys.rs",
"            if sn != c.is_null(row) {\n                return false;\n            }", "            if !sn && c.is_null(row) {\n                return false;\n            }"))
mut("dictionary join key compared by code, not by value", ("src/keys.rs",
"            _ => self.bytes(i) == other.bytes(j),\n        }\n    }\n\n    pub fn same_kind",
"            (KData::Dict { keys: a, .. }, KData::Dict { keys: b, .. }) => a[i] == b[j],\n            _ => self.bytes(i) == other.bytes(j),\n        }\n    }\n\n    pub fn same_kind"))
mut("dictionary key hashed by code, not by value", ("src/keys.rs",
"|i| dh[keys[i] as usize]);", "|i| { let _ = &dh; mix(keys[i] as u64) });"))
mut("output batching: a batch may exceed the cap by one row", ("src/join.rs",
"if pr.len() >= self.cap {", "if pr.len() > self.cap {"))
mut("output batching: a final batch of one row is lost", ("src/join.rs",
"if !pr.is_empty() {\n            out(self.emit(&b, &pr, &br)?)?;\n        }\n        self.hv = hv;", "if pr.len() > 1 {\n            out(self.emit(&b, &pr, &br)?)?;\n        }\n        self.hv = hv;"))
mut("SUM adds the values of NULL rows", ("src/agg.rs",
"state[g * s + 1 + j] += (v[i] as i128) * (ok as i128);", "state[g * s + 1 + j] += v[i] as i128;"))
mut("SUM does not count its NULL inputs (non-NULL count wrong)", ("src/agg.rs",
"nulls[g * kk + j] += !ok as u64;", "nulls[g * kk + j] += 0;"))
mut("empty build side not short-circuited in the probe", ("src/join.rs",
"if n == 0 || table.n == 0 {", "if n == 0 {"))
mut("join key hash ignores the second key column", ("src/keys.rs",
"        let first = ci == 0;", "        if ci > 0 { break; }\n        let first = ci == 0;"))
mut("range filter keeps one extra, rejected row", ("src/kernels.rs",
"    out.truncate(k);\n    out\n}\n\n/// Exact sum", "    out.truncate((k + 1).min(out.len()));\n    out\n}\n\n/// Exact sum"))


def sh(cmd, cwd, env=None):
    e = dict(os.environ)
    e.update(env or {})
    e["CARGO_TARGET_DIR"] = TGT
    e.setdefault("CARGO_BUILD_JOBS", "2")
    try:
        r = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True, env=e, timeout=TIMEOUT)
        return r.returncode, r.stdout + r.stderr
    except subprocess.TimeoutExpired:
        return 124, "timeout"


def fresh_copy():
    os.makedirs(WORK, exist_ok=True)
    for item in COPY:
        src, dst = os.path.join(HERE, item), os.path.join(WORK, item)
        if os.path.isdir(src):
            shutil.rmtree(dst, ignore_errors=True)
            # Fresh mtimes (shutil.copy keeps the mode, e.g. check.sh stays executable, but not the mtime; copy2 keeps both): cargo decides what to rebuild from mtimes, and a restored file that keeps
            # its old mtime looks older than the previous mutant's build, so the mutant's binary would be reused.
            shutil.copytree(src, dst, copy_function=shutil.copy)
        else:
            shutil.copy(src, dst)


def apply(edits):
    fresh_copy()
    for f, old, new in edits:
        p = os.path.join(WORK, f)
        s = open(p).read()
        n = s.count(old)
        if n != 1:
            return f"pattern occurs {n} times (need exactly 1) in {f}: {old[:70]!r}"
        open(p, "w").write(s.replace(old, new))
    return None


LAYERS = [
    ("tests", lambda: sh("cargo test --release -q", WORK)),
    ("nullcheck", lambda: sh(f"{BIN} nullcheck {DATA}", WORK)),
    ("duckdb", lambda: sh(f"./check.sh {DATA}", WORK)),
]


def snapshot():
    out = {}
    for item in COPY:
        base = os.path.join(HERE, item)
        if os.path.isdir(base):
            for d, _, fs in os.walk(base):
                for f in fs:
                    p = os.path.join(d, f)
                    out[p] = open(p, "rb").read()
        else:
            out[base] = open(base, "rb").read()
    return out


def main():
    args = sys.argv[1:]
    if "--list" in args:
        print("\n".join(n for n, _ in M))
        return
    matrix = "--matrix" in args
    dry = "--dry" in args
    only_layer = None
    if "--layer" in args:
        only_layer = args[args.index("--layer") + 1]
    names = [a for a in args if not a.startswith("--") and a != only_layer]
    before = snapshot()
    os.makedirs(MUT, exist_ok=True)
    if not os.path.isdir(os.path.join(TGT, "release")):
        os.makedirs(TGT, exist_ok=True)
        seed = os.path.join(HERE, "target", "release")
        if os.path.isdir(seed):  # reuse already compiled dependencies
            shutil.copytree(seed, os.path.join(TGT, "release"), symlinks=True)
    def chosen(n):
        # An argument that is exactly a mutant name selects only that mutant; otherwise it is a substring.
        return not names or any(n == x if any(m == x for m, _ in M) else x in n for x in names)
    sel = [(n, e) for n, e in M if chosen(n)]
    unknown = [x for x in names if not any(x in n for n, _ in M)]
    if unknown:
        print(f"no mutant matches {unknown}", file=sys.stderr)
        sys.exit(2)
    layers = [l for l in LAYERS if not only_layer or l[0] == only_layer]
    if dry:
        bad = 0
        for n, e in sel:
            err = apply(e)
            print(("BAD  " + err + "  <- ") if err else "ok   ", n)
            bad += bool(err)
        sys.exit(1 if bad else 0)

    # Baseline: the unmutated copy must pass every layer, or nothing below means anything.
    fresh_copy()
    print(f"data: {DATA}; {len(sel)} mutants; layers: {[l[0] for l in layers]}; matrix={matrix}", flush=True)
    t0 = time.time()
    rc, out = sh("cargo build --release -q && cargo test --release --no-run -q", WORK)
    if rc:
        print("baseline does not build:\n" + out[-2000:]); sys.exit(2)
    for ln, fn in layers:
        rc, out = fn()
        if rc:
            print(f"baseline fails the {ln} layer:\n" + out[-3000:]); sys.exit(2)
    print(f"baseline passes all layers ({time.time() - t0:.0f}s)", flush=True)

    results = []
    for n, e in sel:
        t1 = time.time()
        err = apply(e)
        if err:
            results.append((n, "INVALID", err)); print(f"INVALID  {n}: {err}", flush=True); continue
        rc, out = sh("cargo build --release -q && cargo test --release --no-run -q", WORK)
        if rc:
            results.append((n, "INVALID", "does not compile")); print(f"INVALID  {n}: does not compile\n{out[-1500:]}", flush=True); continue
        caught = []
        for ln, fn in layers:
            rc, out = fn()
            if rc:
                caught.append(ln + (" (timeout)" if rc == 124 else ""))
                if not matrix:
                    break
        verdict = "CAUGHT" if caught else "SURVIVED"
        results.append((n, verdict, ", ".join(caught)))
        print(f"{verdict:<8} {n}  [{', '.join(caught) or 'no layer failed'}]  ({time.time() - t1:.0f}s)", flush=True)

    shutil.rmtree(WORK, ignore_errors=True)
    after = snapshot()
    print("\n| # | breakage | result | caught by |\n|---:|---|---|---|")
    for i, (n, v, c) in enumerate(results, 1):
        print(f"| {i} | {n} | {v} | {c} |")
    caught_n = sum(v == "CAUGHT" for _, v, _ in results)
    print(f"\ncaught {caught_n} of {len(results)}; survived {sum(v == 'SURVIVED' for _, v, _ in results)}; invalid {sum(v == 'INVALID' for _, v, _ in results)}")
    unchanged = before == after
    print("real sources unchanged (byte-compared before and after)" if unchanged else "ERROR: the real sources changed during the run")
    sys.exit(0 if caught_n == len(results) and unchanged else 1)


main()
