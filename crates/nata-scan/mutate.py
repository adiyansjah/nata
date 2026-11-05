#!/usr/bin/env python3
"""Mutation check of the pruning rules of nata-scan. Each mutant is ONE exact text edit of src/prune.rs (or another source file), applied to a private copy of
the crate in a private workspace; `cargo test --release` runs on the copy.
  CAUGHT   = a test fails
  SURVIVED = the tests pass: a gap in the TESTS (fix the tests, never weaken the mutant)
  INVALID  = the edit does not apply exactly once, changes nothing, or the copy does not compile; an INVALID mutant counts as neither caught nor missed and fails the run
The unmutated copy runs first and must pass. The real tree is hashed before and after and must not change.
A mutant is one edit, or a list of edits applied together (each must apply exactly once).
The match ignores whitespace, so rustfmt does not break the mutants.
usage: mutate.py [--list] [--dry] [--jobs N] [--work DIR] [--out FILE] [name ...]"""
import argparse, concurrent.futures, hashlib, os, re, shutil, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
TESTS = ["--test", "rules", "--test", "soundness", "--test", "effectiveness"]   # duckdb_oracle never calls the library

P = "src/prune.rs"
S = "src/stats.rs"
M = {
 # --- the six operators: the unsound direction (a bound that equals the literal is wrongly excluded) and the weak direction (kept when it could be skipped)
 "le-treated-as-lt": (P, "CmpOp::Le => lo.as_ref().is_some_and(|l| gt(l, v)),", "CmpOp::Le => lo.as_ref().is_some_and(|l| ge(l, v)),"),
 "lt-treated-as-le": (P, "CmpOp::Lt => lo.as_ref().is_some_and(|l| ge(l, v)),", "CmpOp::Lt => lo.as_ref().is_some_and(|l| gt(l, v)),"),
 "ge-treated-as-gt": (P, "CmpOp::Ge => hi.as_ref().is_some_and(|h| every_value_below(h, hi_inclusive, v)),", "CmpOp::Ge => hi.as_ref().is_some_and(|h| le(h, v)),"),
 "gt-treated-as-ge": (P, "CmpOp::Gt => hi.as_ref().is_some_and(|h| le(h, v)),", "CmpOp::Gt => hi.as_ref().is_some_and(|h| lt(h, v)),"),
 # --- off by one in the range test of `=`
 "eq-prunes-at-the-maximum": (P, "|| hi.as_ref().is_some_and(|h| every_value_below(h, hi_inclusive, v))", "|| hi.as_ref().is_some_and(|h| le(h, v))"),
 "eq-prunes-at-the-minimum": (P, "CmpOp::Eq => { lo.as_ref().is_some_and(|l| gt(l, v)) ||", "CmpOp::Eq => { lo.as_ref().is_some_and(|l| ge(l, v)) ||"),
 "eq-never-prunes-above-the-maximum": (P, "fn every_value_below(h: &Key<'_>, hi_inclusive: bool, v: &Key<'_>) -> bool {\n    if hi_inclusive { lt(h, v) } else { le(h, v) }", "fn every_value_below(h: &Key<'_>, hi_inclusive: bool, v: &Key<'_>) -> bool {\n    if hi_inclusive { le(h, v) && !eq(h, v) && false } else { le(h, v) }"),
 "ne-skips-when-only-the-minimum-equals": (P, "constant_possible && eq(l, h) && eq(l, v)", "constant_possible && eq(l, v)"),
 "ne-needs-no-exact-bounds": (P, "constant_possible && eq(l, h) && eq(l, v)", "eq(l, h) && eq(l, v)"),
 # --- NaN
 # the guard alone: the comparator gives NaN no order, so only the reported reason changes (a unit test pins it)
 "nan-bound-guard-removed": (P, "if nan_bound(&min) || nan_bound(&max) {", "if false && (nan_bound(&min) || nan_bound(&max)) {"),
 # NaN trusted as a number: no guard AND NaN ordered as the largest value (the SQL order)
 "nan-bounds-trusted": [(P, "if nan_bound(&min) || nan_bound(&max) {", "if false && (nan_bound(&min) || nan_bound(&max)) {"),
                        (P, "fn cmp_f64(a: f64, b: f64) -> Option<Ordering> {\n    a.partial_cmp(&b)\n}", "fn cmp_f64(a: f64, b: f64) -> Option<Ordering> {\n    match (a.is_nan(), b.is_nan()) {\n        (true, true) => Some(Ordering::Equal),\n        (true, false) => Some(Ordering::Greater),\n        (false, true) => Some(Ordering::Less),\n        _ => a.partial_cmp(&b),\n    }\n}")],
 "nan-rows-ignored-for-all-three": (P, "if matches!(op, CmpOp::Gt | CmpOp::Ge | CmpOp::Ne) && !nan_free {", "if false && matches!(op, CmpOp::Gt | CmpOp::Ge | CmpOp::Ne) && !nan_free {"),
 "nan-rows-ignored-by-gt": (P, "if matches!(op, CmpOp::Gt | CmpOp::Ge | CmpOp::Ne) && !nan_free {", "if matches!(op, CmpOp::Ge | CmpOp::Ne) && !nan_free {"),
 "nan-rows-ignored-by-ge": (P, "if matches!(op, CmpOp::Gt | CmpOp::Ge | CmpOp::Ne) && !nan_free {", "if matches!(op, CmpOp::Gt | CmpOp::Ne) && !nan_free {"),
 "nan-rows-ignored-by-ne": (P, "if matches!(op, CmpOp::Gt | CmpOp::Ge | CmpOp::Ne) && !nan_free {", "if matches!(op, CmpOp::Gt | CmpOp::Ge) && !nan_free {"),
 "unknown-nan-count-means-no-nan": (P, "let nan_free = col.nan_count == Some(0);", "let nan_free = col.nan_count.is_none_or(|n| n == 0);"),
 "nan-count-zero-ignored": (P, "let nan_free = col.nan_count == Some(0);", "let nan_free = false;"),
 "nan-count-above-rows-trusted": (P, "if col.nan_count.is_some_and(|n| n > non_null) {", "if false && col.nan_count.is_some_and(|n| n > non_null) {"),
 "nan-literal-gt-kept": (P, "CmpOp::Gt => Decision::Skip(SkipReason::Unsatisfiable),", "CmpOp::Gt => Decision::Keep(KeepReason::NanRows),"),
 "nan-literal-le-skipped-when-nan-free": (P, "CmpOp::Eq | CmpOp::Ge => {", "CmpOp::Eq | CmpOp::Ge | CmpOp::Le => {"),
 "nan-literal-eq-ignores-the-nan-count": (P, "                    if nan_free {\n                        Decision::Skip(SkipReason::NoNan)", "                    if true {\n                        Decision::Skip(SkipReason::NoNan)"),
 # --- zeros
 "zero-signs-distinct": (P, "fn cmp_f64(a: f64, b: f64) -> Option<Ordering> {\n    a.partial_cmp(&b)\n}", "fn cmp_f64(a: f64, b: f64) -> Option<Ordering> {\n    Some(a.total_cmp(&b))\n}"),
 "float32-column-literal-narrowed-to-f32": (P, "(ColumnType::Float32 | ColumnType::Float64, Value::Float64(x)) => Some(Key::Float(*x)),", "(ColumnType::Float32 | ColumnType::Float64, Value::Float64(x)) => Some(Key::Float(f64::from(*x as f32))),"),
 # --- truncated bounds
 "truncated-bounds-treated-as-exact": (P, "Some(m) => match (m.key, m.exact) {", "Some(m) => match (m.key, true) {"),
 "constant-ignores-exactness": (P, "constant_possible && eq(l, h) && eq(l, v)", "eq(l, h) && eq(l, v)"),
 "increment-wraps-0xff": (P, "if let Some(next) = last.checked_add(1) {", "if let Some(next) = Some(last.wrapping_add(1)) {"),
 "increment-gives-up-on-0xff": (P, "while let Some(last) = v.pop() {\n        if let Some(next)", "while let Some(last) = v.pop() {\n        if last == 0xFF {\n            return None;\n        }\n        if let Some(next)"),
 "truncated-maximum-used-without-increment-but-exclusive": (P, "(incremented.as_deref().map(Key::Bytes), false)", "(Some(Key::Bytes(bytes)), false)"),
 "utf8-bound-accepted-when-cut": (S, "let s = std::str::from_utf8(bytes).ok()?; Some(Bound { value: Value::Utf8(s.to_string()), exact, })", "Some(Bound { value: Value::Utf8(String::from_utf8_lossy(bytes).into_owned()), exact, })"),
 "bytes-compared-as-signed": (P, "(Key::Bytes(x), Key::Bytes(y)) => Some(x.cmp(y)),", "(Key::Bytes(x), Key::Bytes(y)) => Some(x.iter().map(|b| i8::from_ne_bytes([*b])).cmp(y.iter().map(|b| i8::from_ne_bytes([*b])))),"),
 # --- missing statistics
 "missing-bound-treated-as-zero": (P, "        None => Ok(None),\n        Some(b) => match stat_key(ty, &b.value) {", "        None => Ok(stat_key(ty, &Value::Int64(0)).or(stat_key(ty, &Value::Int32(0))).or(stat_key(ty, &Value::Float64(0.0))).map(|key| Stat { key, exact: true })),\n        Some(b) => match stat_key(ty, &b.value) {"),
 "missing-null-count-treated-as-zero": (P, "        n => n,\n    };\n    let all_null", "        n => n.or(Some(0)),\n    };\n    let all_null"),
 # --- null counts
 "null-count-ignored-all-null-kept": (P, "            if all_null {\n                return Decision::Skip(SkipReason::AllNull);", "            if false {\n                return Decision::Skip(SkipReason::AllNull);"),
 "is-not-null-keeps-all-null": (P, "} else if all_null {", "} else if false && all_null {"),
 "is-null-never-skipped": (P, "Some(0) => Decision::Skip(SkipReason::NoNulls),", "Some(0) => Decision::Keep(KeepReason::MayMatch),"),
 "is-null-skipped-unless-all-null": (P, "Some(0) => Decision::Skip(SkipReason::NoNulls),", "Some(n) if n < rows => Decision::Skip(SkipReason::NoNulls),"),
 "all-null-skipped-for-is-null": (P, "        Term::IsNull { .. } => match nulls {", "        Term::IsNull { .. } if all_null => Decision::Skip(SkipReason::AllNull),\n        Term::IsNull { .. } => match nulls {"),
 "null-count-above-rows-trusted": (P, "Some(n) if n > rows => return Decision::Keep(KeepReason::UnsoundStatistics),", "Some(n) if false && n > rows => return Decision::Keep(KeepReason::UnsoundStatistics),"),
 "all-null-with-bounds-trusted": (P, "let contradiction = all_null && (col.min.is_some() || col.max.is_some());", "let contradiction = false;"),
 "is-not-null-skipped-when-some-null": (P, "} else if all_null {", "} else if nulls.is_some_and(|n| n > 0) {"),
 "exclusive-upper-bound-off-by-one": (P, "if hi_inclusive { lt(h, v) } else { le(h, v) }\n}", "if hi_inclusive { lt(h, v) } else { lt(h, v) }\n}"),
 "empty-row-group-kept": (P, "    if stats.num_rows == 0 {\n        return Decision::Skip(SkipReason::EmptyRowGroup);", "    if false && stats.num_rows == 0 {\n        return Decision::Skip(SkipReason::EmptyRowGroup);"),
 # --- contradictions
 "min-above-max-trusted": (P, "        && gt(&lo.key, &hi.key)\n", "        && false && gt(&lo.key, &hi.key)\n"),
 "bound-of-wrong-type-ignored": (P, "            None => Err(()),", "            None => Ok(None),"),
 # --- types and decimals
 "int-literal-truncated-to-32-bits": (P, "(ColumnType::Int32 | ColumnType::Int64, Value::Int64(x)) => Some(Key::Int(i128::from(*x))),", "(ColumnType::Int32 | ColumnType::Int64, Value::Int64(x)) => Some(Key::Int(i128::from(*x as i32))),"),
 "decimal-literal-precision-unchecked": (P, "scale, }, ) if decimal_fits(*unscaled, *precision, *scale) => Some(Key::Dec(*unscaled, *scale)),", "scale, }, ) => Some(Key::Dec(*unscaled, *scale)),"),
 "decimal-literal-rounded-to-the-column-scale": (P, "        Ordering::Less => match rescale(a, sb.saturating_sub(sa)) {\n            Some(a2) => a2.cmp(&b),\n            None => sign_order(a),\n        },", "        Ordering::Less => a.cmp(&b.checked_div(10i128.pow(u32::from(sb.saturating_sub(sa)))).unwrap_or(0)),"),
 "decimal-rescale-wraps": (P, "10i128.checked_pow(u32::from(k)).and_then(|p| x.checked_mul(p))", "Some(x.wrapping_mul(10i128.wrapping_pow(u32::from(k))))"),
 "decimal-overflow-sign-flipped": (P, "if overflowed > 0 { Ordering::Greater } else { Ordering::Less }", "if overflowed > 0 { Ordering::Less } else { Ordering::Greater }"),
 "decimal-bound-scale-unchecked": (P, "if p == &precision && s == &scale && decimal_fits(*unscaled, *p, *s) => { Some(Key::Dec(*unscaled, *s)) }", "if decimal_fits(*unscaled, *p, *s) => { Some(Key::Dec(*unscaled, *s)) }"),
 "float-literal-accepted-for-decimal-column": (P, "        (ColumnType::Decimal { .. }, _) => None,", "        (ColumnType::Decimal { .. }, Value::Float64(x)) => Some(Key::Dec(*x as i128, 0)),\n        (ColumnType::Decimal { .. }, _) => None,"),
 "unsupported-column-pruned": (P, "    if matches!(col.ty, ColumnType::Unsupported) || !column_type_is_valid(col.ty) {\n        return Decision::Keep(KeepReason::UnsupportedType);\n    }", "    if !column_type_is_valid(col.ty) {\n        return Decision::Keep(KeepReason::UnsupportedType);\n    }"),
 # --- conjunction
 "conjunction-never-skips": (P, "Decision::Skip(reason) => return Decision::Skip(reason),", "Decision::Skip(_) => {}"),
}


def sha_tree():
    h = hashlib.sha256()
    for d, _, fs in sorted(os.walk(HERE)):
        if "target" in d.split(os.sep) or "mutwork" in d:
            continue
        for f in sorted(fs):
            if f.endswith((".rs", ".toml", ".py", ".md")):
                h.update(open(os.path.join(d, f), "rb").read())
    return h.hexdigest()


def stage(work, name):
    """A private workspace: the real root manifest (lints, release profile) with only this crate as a member, and a copy of the crate."""
    base = os.path.join(work, name)
    shutil.rmtree(base, ignore_errors=True)
    os.makedirs(os.path.join(base, "crates"))
    root = open(os.path.join(ROOT, "Cargo.toml")).read()
    root = re.sub(r"members = \[[^\]]*\]", 'members = ["crates/nata-scan"]', root)
    root = re.sub(r"exclude = \[[^\]]*\]\n", "", root)
    open(os.path.join(base, "Cargo.toml"), "w").write(root)
    dst = os.path.join(base, "crates", "nata-scan")
    os.makedirs(dst)
    for x in ["src", "tests", "Cargo.toml"]:
        s = os.path.join(HERE, x)
        if os.path.isdir(s):
            shutil.copytree(s, os.path.join(dst, x))
        else:
            shutil.copy(s, os.path.join(dst, x))
    return base, dst


def cargo(base, work, name, args):
    env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(work, "target-" + name), CARGO_TERM_COLOR="never")
    p = subprocess.run(["cargo"] + args, cwd=base, env=env, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


def run_one(args):
    n, work, dry, slot = args
    edits = M[n] if isinstance(M[n], list) else [M[n]]
    base, dst = stage(work, "slot%d" % slot)
    for f, old, new in edits:
        p = os.path.join(dst, f)
        t = open(p).read()
        # whitespace-insensitive match: rustfmt may wrap the lines
        rx = re.compile(r"\s*".join(re.escape(c) for c in old if not c.isspace()))
        found = list(rx.finditer(t))
        if len(found) != 1:
            return n, "INVALID", "edit applies %d times" % len(found)
        if re.sub(r"\s+", "", old) == re.sub(r"\s+", "", new):
            return n, "INVALID", "edit changes nothing"
        open(p, "w").write(t[: found[0].start()] + new + t[found[0].end():])
    cmd = ["check", "--release", "--offline", "-p", "nata-scan", "--tests"] if dry else ["test", "--release", "--offline", "--no-fail-fast", "-p", "nata-scan"] + TESTS
    rc, out = cargo(base, work, "slot%d" % slot, cmd)
    if "error[" in out or "could not compile" in out or re.search(r"^error: ", out, re.M) and rc != 0 and "test result" not in out:
        return n, "INVALID", "does not compile: " + " ".join(l for l in out.splitlines() if l.startswith("error"))[:200]
    if dry:
        return n, "compiles", ""
    if rc == 0:
        return n, "SURVIVED", "the tests cannot see this bug"
    failed = sorted(set(l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")))
    return n, "CAUGHT", ", ".join(failed) or "a failing test"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--dry", action="store_true")
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--work", default=os.path.join(HERE, "mutwork"))
    ap.add_argument("--out")
    a = ap.parse_args()
    if a.list:
        print("\n".join(M))
        return 0
    for n in a.names:
        if n not in M:
            print("unknown mutant", n)
            return 2
    os.makedirs(a.work, exist_ok=True)
    before = sha_tree()
    t0 = time.time()
    base, _ = stage(a.work, "baseline")
    rc, out = cargo(base, a.work, "baseline", ["test", "--release", "--offline", "--no-fail-fast", "-p", "nata-scan"] + TESTS)
    if rc != 0:
        print("BASELINE FAILED\n" + out[-3000:])
        return 1
    print("baseline ok (%.0fs)" % (time.time() - t0), flush=True)
    names = a.names or list(M)
    results = {}
    # one slot (private workspace and target dir) per worker; a mutant runs in the slot of its index modulo jobs, sequentially within a slot
    slots = [[n for i, n in enumerate(names) if i % a.jobs == s] for s in range(a.jobs)]

    def worker(s):
        return [run_one((n, a.work, a.dry, s)) for n in slots[s]]

    with concurrent.futures.ThreadPoolExecutor(a.jobs) as ex:
        for res in ex.map(worker, range(a.jobs)):
            for n, status, detail in res:
                results[n] = (status, detail)
    lines = []
    for n in names:
        status, detail = results[n]
        lines.append("%-9s %s: %s" % (status, n, detail))
    print("\n".join(lines))
    counts = {k: sum(1 for s, _ in results.values() if s == k) for k in ("CAUGHT", "SURVIVED", "INVALID", "compiles")}
    print("\nmutants: %d; %s; %.0fs" % (len(names), ", ".join("%s %d" % (k, v) for k, v in counts.items() if v), time.time() - t0))
    if a.out:
        open(a.out, "w").write("\n".join(lines) + "\n")
    if sha_tree() != before:
        print("THE REAL TREE CHANGED DURING THE RUN")
        return 1
    return 0 if counts["CAUGHT"] == len(names) or a.dry and counts["compiles"] == len(names) else 1


sys.exit(main())
