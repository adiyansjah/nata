#!/usr/bin/env python3
"""Mutation check of the Iceberg transforms and the scan planner: each mutant is ONE text edit of the source, applied to a private copy of the crate;
`cargo test --profile mutate --test transforms --test soundness` runs on the copy. CAUGHT = a test fails; SURVIVED = the tests pass (a gap); INVALID = the
edit does not apply exactly once or the copy does not compile. The unmutated copy runs first and must pass.
usage: mutate.py [--list] [--dry] [--work DIR] [name ...]"""
import argparse, hashlib, os, shutil, subprocess, sys
HERE = os.path.dirname(os.path.abspath(__file__))
M = {
 "truncate-remainder-can-be-negative": ("src/transform.rs", "Int(x) => Int(x.wrapping_sub(x.rem_euclid(*w as i32))),", "Int(x) => Int(x.wrapping_sub(*x % (*w as i32))),"),
 "day-truncates-toward-zero": ("src/transform.rs", "Ts(t) => (t.div_euclid(MICROS_PER_DAY), Some(*t))", "Ts(t) => (t / MICROS_PER_DAY, Some(*t))"),
 "hour-truncates-toward-zero": ("src/transform.rs", "_ => Int(micros.expect(\"hour needs a timestamp\").div_euclid(MICROS_PER_HOUR) as i32),", "_ => Int((micros.expect(\"hour needs a timestamp\") / MICROS_PER_HOUR) as i32),"),
 "bucket-keeps-the-sign": ("src/transform.rs", "Int(((iceberg_hash(v) & i32::MAX) as i64 % *n as i64) as i32)", "Int((iceberg_hash(v) as i64 % *n as i64) as i32)"),
 "bucket-hashes-int-as-int": ("src/transform.rs", "Int(x) | Date(x) => hash_long(*x as i64),", "Int(x) | Date(x) => murmur3_32(&x.to_le_bytes(), 0) as i32,"),
 "time-projection-lt-is-strict": ("src/transform.rs", "                Op::Lt | Op::Le => Some((Op::Le, self.apply(lit)?)),\n                Op::Gt | Op::Ge => Some((Op::Ge, self.apply(lit)?)),\n                Op::StartsWith => None,", "                Op::Lt | Op::Le => Some((Op::Lt, self.apply(lit)?)),\n                Op::Gt | Op::Ge => Some((Op::Ge, self.apply(lit)?)),\n                Op::StartsWith => None,"),
 "truncate-projection-gt-is-strict": ("src/transform.rs", "                Op::Gt | Op::Ge => Some((Op::Ge, self.apply(lit)?)),\n                Op::StartsWith => match lit {", "                Op::Gt | Op::Ge => Some((Op::Gt, self.apply(lit)?)),\n                Op::StartsWith => match lit {"),
 "bucket-projects-ranges": ("src/transform.rs", "Transform::Bucket(_) => (op == Op::Eq).then(|| (Op::Eq, self.apply(lit).unwrap())),", "Transform::Bucket(_) => Some((op, self.apply(lit).unwrap())),"),
 "le-prunes-equal-lower-bound": ("src/expr.rs", "Op::Le => b.lower.as_ref().is_none_or(|l| vcmp(l, &lo_lit).is_le()),", "Op::Le => b.lower.as_ref().is_none_or(|l| vcmp(l, &lo_lit).is_lt()),"),
 "ge-prunes-equal-upper-bound": ("src/expr.rs", "Op::Ge => nan_possible || b.upper.as_ref().is_none_or(|u| vcmp(u, &up_lit).is_ge()),", "Op::Ge => nan_possible || b.upper.as_ref().is_none_or(|u| vcmp(u, &up_lit).is_gt()),"),
 "eq-prunes-equal-upper-bound": ("src/expr.rs", "&& b.upper.as_ref().is_none_or(|u| vcmp(u, &up_lit).is_ge()),\n                Op::StartsWith", "&& b.upper.as_ref().is_none_or(|u| vcmp(u, &up_lit).is_gt()),\n                Op::StartsWith"),
 "nan-rows-are-ignored-by-gt": ("src/expr.rs", "Op::Gt => nan_possible || b.upper", "Op::Gt => b.upper"),
 "zero-literal-not-widened": ("src/expr.rs", "let (lo_lit, up_lit) = (widen(lit, false), widen(lit, true));", "let (lo_lit, up_lit) = (lit.clone(), lit.clone());"),
 "negated-comparison-forgets-nan": ("src/expr.rs", "P::Cmp(Op::Gt, v) => or(cmp(c, Op::Le, v.clone()), pred(c, P::IsNaN)),", "P::Cmp(Op::Gt, v) => cmp(c, Op::Le, v.clone()),"),
 "truncate-overflow-guard-removed": ("src/expr.rs", "Some(Some(v)) if guard.is_some_and(|w| !crate::transform::plausible_truncate_value(v, w)) => true,", ""),
 "string-upper-bound-not-incremented": ("src/table.rs", "increment_str(&hi.chars().take(TRUNC).collect::<String>()).map(Value::Str)", "Some(Value::Str(hi.chars().take(TRUNC).collect()))"),
 "binary-upper-bound-not-incremented": ("src/table.rs", "increment_bytes(&hi[..TRUNC]).map(Value::Bin)", "Some(Value::Bin(hi[..TRUNC].to_vec()))"),
 "manifest-summary-lower-bound-is-the-max": ("src/manifest.rs", "Some(v) => { if lo.as_ref().is_none_or(|l| vcmp(v, l).is_lt()) { lo = Some(v.clone()); }", "Some(v) => { if lo.as_ref().is_none_or(|l| vcmp(v, l).is_gt()) { lo = Some(v.clone()); }"),
 "equality-delete-applies-to-same-commit": ("src/plan.rs", "v.iter().filter(|(ds, _)| *ds > seq).map(|(_, d)| d.clone()).collect()).unwrap_or_default();", "v.iter().filter(|(ds, _)| *ds >= seq).map(|(_, d)| d.clone()).collect()).unwrap_or_default();"),
 "position-delete-misses-same-commit": ("src/plan.rs", "v.iter().filter(|(ds, d)| *ds >= seq &&", "v.iter().filter(|(ds, d)| *ds > seq &&"),
 "global-equality-delete-also-needs-partition": ("src/plan.rs", "eq_deletes.extend(eq_global.iter().filter(|(ds, _)| *ds > seq).map(|(_, d)| d.clone()));", ""),
 "projection-uses-the-default-spec": ("src/plan.rs", "let proj = &projections[&spec.id];", "let proj = &projections[&meta.default_spec_id];"),
}
def sha_tree():
    h = hashlib.sha256()
    for d, _, fs in sorted(os.walk(HERE)):
        if "target" in d.split(os.sep) or "mutwork" in d: continue
        for f in sorted(fs):
            if f.endswith((".rs", ".toml", ".py", ".sh")): h.update(open(os.path.join(d, f), "rb").read())
    return h.hexdigest()
def stage(work):
    dst = os.path.join(work, "copy"); shutil.rmtree(dst, ignore_errors=True); os.makedirs(dst)
    for x in ["src", "tests", "Cargo.toml", "Cargo.lock"]:
        s = os.path.join(HERE, x)
        (shutil.copytree if os.path.isdir(s) else shutil.copy)(s, os.path.join(dst, x), **({"copy_function": shutil.copy} if os.path.isdir(s) else {}))
    return dst
def cargo(dst, work, args):
    env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(work, "target"))
    p = subprocess.run(["cargo"] + args, cwd=dst, env=env, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr
def main():
    ap = argparse.ArgumentParser(); ap.add_argument("names", nargs="*"); ap.add_argument("--list", action="store_true"); ap.add_argument("--dry", action="store_true")
    ap.add_argument("--work", default=os.path.join(HERE, "mutwork")); a = ap.parse_args()
    if a.list: print("\n".join(M)); return 0
    os.makedirs(a.work, exist_ok=True); before = sha_tree(); fails = 0
    run = lambda d: cargo(d, a.work, ["check", "--profile", "mutate", "--tests"] if a.dry else ["test", "--profile", "mutate", "--no-fail-fast", "--test", "transforms", "--test", "soundness"])
    dst = stage(a.work); rc, out = run(dst)
    if rc != 0: print("BASELINE FAILED\n" + out[-3000:]); return 1
    print("baseline ok", flush=True)
    for n in (a.names or list(M)):
        f, old, new = M[n]; dst = stage(a.work); p = os.path.join(dst, f); t = open(p).read()
        if t.count(old) != 1: print(f"INVALID  {n}: edit applies {t.count(old)} times"); fails += 1; continue
        open(p, "w").write(t.replace(old, new)); rc, out = run(dst)
        if a.dry: print(("compiles " if rc == 0 else "INVALID  ") + n); fails += rc != 0; continue
        if "error[" in out or "could not compile" in out: print(f"INVALID  {n}: does not compile"); fails += 1
        elif rc == 0: print(f"SURVIVED {n}: the tests cannot see this bug"); fails += 1
        else:
            failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
            print(f"CAUGHT   {n}: by {', '.join(failed) or 'a failing test'}")
        sys.stdout.flush()
    if sha_tree() != before: print("THE REAL TREE CHANGED DURING THE RUN"); fails += 1
    return 1 if fails else 0
sys.exit(main())
