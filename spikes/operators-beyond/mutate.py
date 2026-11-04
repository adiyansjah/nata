#!/usr/bin/env python3
"""Mutation check of the operators-beyond spike: each mutant is one text edit of a source file, applied to a PRIVATE COPY of the crate;
`cargo test --profile mutate --test ops` runs on the copy. CAUGHT = the tests fail; SURVIVED = they pass (a gap in the tests: a failure);
INVALID = an edit does not apply exactly once or the copy does not compile (a bug in the mutant, never evidence).
The unmutated copy runs first and must pass. The real tree is hashed before and after and must be unchanged.
usage: mutate.py [--list] [--dry] [--work DIR] [name ...]     (--dry: apply every edit and `cargo check` the copy, no tests)"""
import argparse, hashlib, os, shutil, subprocess, sys
HERE = os.path.dirname(os.path.abspath(__file__))
M = {
 "sort-nulls-flipped": ("src/sort.rs", "nulls_first: k.nulls_first }", "nulls_first: !k.nulls_first }"),
 "sort-desc-ignored": ("src/sort.rs", "descending: k.desc,", "descending: false,"),
 "merge-drops-row-at-batch-end": ("src/sort.rs", "if *pos == b.num_rows() {", "if *pos + 1 == b.num_rows() {"),
 "spill-skips-last-chunk": ("src/sort.rs", "        for c in sorted {\n            w.write(c)?;", "        for c in sorted.iter().take(sorted.len().saturating_sub(1)) {\n            w.write(c)?;"),
 "spill-file-not-deleted": ("src/sort.rs", "                drop(r);\n                remove(&p);", "                drop(r);\n                let _ = &p;"),
 "spill-reservation-not-released": ("src/sort.rs", "        lock(&self.sh.runs).push(Run::File(p));\n        self.sh.budget.release(self.bytes);\n        self.buf.clear();\n        self.bytes = 0;", "        lock(&self.sh.runs).push(Run::File(p));\n        self.buf.clear();\n        self.bytes = 0;"),
 "topn-off-by-one": ("src/sort.rs", "Ok(vec![s.slice(0, n.min(s.num_rows()))])", "Ok(vec![s.slice(0, n.saturating_sub(1).min(s.num_rows()))])"),
 "join-null-build-key-matches": ("src/join.rs", "            if keys.is_valid(i) {\n                next[i]", "            if true {\n                next[i]"),
 "join-null-probe-key-matches": ("src/join.rs", "            if pk.is_valid(i) {\n                let mut j", "            if true {\n                let mut j"),
 "anti-drops-unmatched": ("src/join.rs", "                    JoinKind::Anti => pi.push(i as u32),", "                    JoinKind::Anti => {}"),
 "left-forgets-unmatched": ("src/join.rs", "                    JoinKind::Left | JoinKind::Full => {\n                        pi.push", "                    JoinKind::Full => {\n                        pi.push"),
 "residual-ignored": ("src/join.rs", "(Some(rp), Some(rbk)) => rp.is_valid(i) && rbk.is_valid(j as usize) && rp.value(i) != rbk.value(j as usize),", "(Some(_), Some(_)) => true,"),
 "residual-null-passes": ("src/join.rs", "(Some(rp), Some(rbk)) => rp.is_valid(i) && rbk.is_valid(j as usize) && rp.value(i) != rbk.value(j as usize),", "(Some(rp), Some(rbk)) => !(rp.is_valid(i) && rbk.is_valid(j as usize) && rp.value(i) == rbk.value(j as usize)),"),
 "full-does-not-track-matches": ("src/join.rs", "let track = matches!(spec.kind, JoinKind::Right | JoinKind::Full);", "let track = matches!(spec.kind, JoinKind::Right);"),
 "probe-loses-extra-output": ("src/join.rs", "Ok(if self.pending.is_empty() { OpResult::NeedMoreInput } else { OpResult::HaveMoreOutput })", "Ok(OpResult::NeedMoreInput)"),
 "grace-skips-last-partition": ("src/join.rs", "if p >= self.sh.build.nparts {", "if p + 1 >= self.sh.build.nparts {"),
 "grace-forgets-unmatched-build": ("src/join.rs", "            self.queue.extend(t.unmatched_build()?);\n            Ok(())", "            Ok(())"),
 "window-rank-is-row-number": ("src/window.rs", "(rk[i], dr[i]) = (rk[i - 1], dr[i - 1]);", "(rk[i], dr[i]) = (rn[i], dr[i - 1] + 1);"),
 "window-splits-partitions-across-batches": ("src/window.rs", "while s > 0 && rows.row(s - 1) == rows.row(n - 1) {", "while false {"),
 "agg-all-null-sum-is-zero": ("src/agg.rs", "g.valid[k][dst] |= self.t.valid[k][src];", "g.valid[k][dst] = true;"),
 "agg-count-counts-nulls": ("src/agg.rs", "self.t.sums[k][g] += col.is_valid(i) as i128;", "self.t.sums[k][g] += 1;"),
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
    dst = stage(a.work)
    rc, out = cargo(dst, a.work, ["check", "--profile", "mutate", "--tests"] if a.dry else ["test", "--profile", "mutate", "--test", "ops"])
    if rc != 0: print("BASELINE FAILED\n" + out[-3000:]); return 1
    print("baseline ok", flush=True)
    for n in (a.names or list(M)):
        f, old, new = M[n]; dst = stage(a.work); p = os.path.join(dst, f); t = open(p).read()
        if t.count(old) != 1: print(f"INVALID  {n}: edit applies {t.count(old)} times"); fails += 1; continue
        open(p, "w").write(t.replace(old, new))
        rc, out = cargo(dst, a.work, ["check", "--profile", "mutate", "--tests"] if a.dry else ["test", "--profile", "mutate", "--test", "ops"])
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
