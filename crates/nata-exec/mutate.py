#!/usr/bin/env python3
"""Mutation check of the executor (nata-exec, the CSV writer of nata-parquet, the `nata sql` command). Each mutant is ONE exact text edit (or a list of edits
applied together) of a source file; the tests run on a private copy of the three crates in a private workspace (the other crates are symlinked).
  CAUGHT   = a test fails
  SURVIVED = the tests pass: a gap in the TESTS (fix the tests, never weaken the mutant)
  INVALID  = the edit does not apply exactly once, changes nothing, or the copy does not compile; an INVALID mutant counts as neither caught nor missed
             and fails the run
Suites: `exec` runs `cargo test -p nata-exec` (unit tests, tests/exec.rs, the in-process scenarios); `cli` runs the `sql_cli` and
`sql_reliability` tests of the `nata` crate (the process: exit codes, output, SIGINT, closed stdout). A `cli` mutant is run with the `cli` suite only.
The unmutated copy runs first and must pass. The real tree is hashed before and after and must not change. The match ignores whitespace, so rustfmt does
not break the mutants.
usage: mutate.py [--list] [--work DIR] [--out FILE] [name ...]"""
import argparse, hashlib, os, re, shutil, subprocess, sys, time
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
EV, CA, NU, EX, VA, EXP, OUT = ("crates/nata-exec/src/" + f for f in ("eval.rs", "cast.rs", "num.rs", "exec.rs", "value.rs", "explain.rs", "output.rs"))
CSV = "crates/nata-parquet/src/csv.rs"
SQL = "crates/nata/src/sql.rs"

# name: (suite, [(file, old, new), ...])
AG = "crates/nata-exec/src/aggregate.rs"
SPILL = "crates/nata-exec/src/spill/registry.rs"
IPC = "crates/nata-exec/src/spill/ipc.rs"
PIPE = "crates/nata-exec/src/aggregate/pipeline.rs"
STORAGE = "crates/nata-exec/src/aggregate/storage.rs"
M = {
 "aggregate-cancel-poll-removed": ("exec", [(AG, "(evaluated.next - start).is_multiple_of(CANCEL_ROWS) && cancelled()", "(evaluated.next - start).is_multiple_of(CANCEL_ROWS) && false && cancelled()")]),
 "aggregate-cancel-poll-per-row": ("exec", [(AG, "(evaluated.next - start).is_multiple_of(CANCEL_ROWS) && cancelled()", "(evaluated.next - start).is_multiple_of(1) && cancelled()")]),
 "canonical-hash-recursion-seed-lost": ("exec", [("crates/nata-exec/src/aggregate/hashing.rs", "SEED ^ u64::from(depth)", "SEED ^ u64::from(depth).min(0)")]),
 "owner-initialization-cleanup-skipped": ("exec", [(SPILL, "let _ = fs::remove_dir(&directory);", "// abandoned initialization directory")]),
 "restart-cleanup-skipped": ("exec", [("crates/nata-exec/src/spill/cleanup.rs", "let mut removed = 0;", 'return Ok(0); #[allow(unreachable_code, reason = "mutation")] let mut removed = 0;')]),
 "spill-recursion-bound-removed": ("exec", [(PIPE, "if depth >= 8 {", "if depth >= 8 && false {")]),
 "spill-partition-not-read-back": ("exec", [(IPC, "let remaining = file.size()?.checked_sub(file.stream_position()?).ok_or_else(invalid)?;", "if file.size()? > 0 { return Ok(None); } let remaining = file.size()?.checked_sub(file.stream_position()?).ok_or_else(invalid)?;")]),
 "spill-reservation-not-released": ("exec", [(STORAGE, "drop(table.grants);", "std::mem::forget(table.grants);")]),
 "spill-descriptor-cap-removed": ("exec", [(SPILL, "if s.stats.open >= self.descriptor_cap {", "if s.stats.open >= self.descriptor_cap && false {")]),
 "spill-cleanup-skipped-on-error": ("exec", [(SPILL, "if allowed != amount { return Err(io::Error::from_raw_os_error(28)); }", "if allowed != amount { std::mem::forget(Arc::clone(&self.owner)); return Err(io::Error::from_raw_os_error(28)); }")]),
 "aggregate-partition-double-counted": ("exec", [(AG, 'for ((dest, source), a) in dest.iter_mut().zip(states).zip(&a.aggregates) { dest.merge(source, a)?; }', 'for ((dest, source), a) in dest.iter_mut().zip(states).zip(&a.aggregates) { dest.merge(source.clone(), a)?; dest.merge(source, a)?; }')]),
 "aggregate-having-before-merge": ("exec", [(PIPE, 'for tables in partials {', r'''for mut tables in partials {
    let a = shared.plan.aggregate.as_ref().expect("aggregate");
    if let Some(h) = &a.having {
        let types: Vec<_> = a.keys.iter().map(|e| e.ty).chain(a.aggregates.iter().map(|e| e.ty)).collect();
        for t in &mut tables {
            let entries = std::mem::take(&mut t.groups.entries);
            t.groups = Groups::new();
            for (key, states) in entries {
                let finalized = states.iter().zip(&a.aggregates).map(|(s,a)| State::Final(s.finish(a).expect("final"))).collect();
                let b = batch(&[(key.clone(), finalized)], a).expect("batch");
                if Evaluator::new(&b, &types).keep_mask(h).expect("having")[0] { t.groups.insert(key, states); }
            }
        }
    }
''')]),
 "bare-projection-allocates-selection": ("exec", [(EX, "let mut all = None;", "let mut all = Some(ev.all_rows());")]),
 # ---- the mutants the card asks for
 "case-evaluates-every-branch": ("exec", [(EV, "let rv = self.eval(result, &taken)?;", "let _all = self.eval(result, sel)?; let rv = self.eval(result, &taken)?;")]),
 "error-raised-for-an-unselected-row": ("exec", [(EV, "for &row in sel { let i = row as usize; if !(l.valid[i] && r.valid[i]) { continue; } let (x, y) = (a[i], b[i]);",
                                                      "for row in 0..n as u32 { let i = row as usize; if !(l.valid[i] && r.valid[i]) { continue; } let (x, y) = (a[i], b[i]);")]),
 "null-treated-as-false-inside-not": ("exec", [(EV, "out.valid[i] = c.valid[i]; d[i] = !v[i];", "out.valid[i] = true; d[i] = !(c.valid[i] && v[i]);")]),
 "offset-off-by-one": ("exec", [(EX, "Some(l) => (l.offset, Some(l.offset.saturating_add(l.n))),", "Some(l) => (l.offset.saturating_add(1), Some(l.offset.saturating_add(l.n))),")]),
 "limit-not-stopping-the-scan": ("exec", [
     (EX, "run_parallel_with(scan, pcfg, limit, cancel,", "run_parallel_with(scan, pcfg, None, cancel,"),
     (EX, "rows_out = rows_out.saturating_add(u64::try_from(part.num_rows()).unwrap_or(u64::MAX)); sink(&part)",
          "let part = match limit { Some(l) => { let room = usize::try_from(l.saturating_sub(offset).saturating_sub(rows_out)).unwrap_or(usize::MAX); if room == 0 { return Ok(()); } part.slice(0, room.min(part.num_rows())) } None => part }; rows_out = rows_out.saturating_add(u64::try_from(part.num_rows()).unwrap_or(u64::MAX)); sink(&part)")]),
 "projection-reads-the-wrong-column": ("exec", [(EX, "arrays.push(Arc::clone(batch.columns().get(*i)", "arrays.push(Arc::clone(batch.columns().get(i.saturating_sub(1))")]),
 "csv-writer-prints-null-as-an-empty-string": ("exec", [(CSV, "None => buf.extend_from_slice(b\"NULL\"),", "None => buf.extend_from_slice(b\"\"),")]),
 # ---- exact numbers
 "decimal-division-truncates": ("exec", [(NU, "if twice >= d.unsigned_abs() {", "if false {")]),
 "decimal-division-by-zero-unchecked-as-zero": ("exec", [(EV, "if b == 0 { return Err(DecFail::DivZero); } // a / 10^sa", "if b == 0 { return Ok(0); } // a / 10^sa")]),
 "decimal-addition-operand-fit-unchecked": ("exec", [(EV, "if !fits(x, p) || !fits(y, p) { return Err(DecFail::Overflow); }", "")]),
 "decimal-result-digits-unchecked": ("exec", [(EV, "if fits(result, p) { Ok(result) } else { Err(DecFail::Overflow) }", "Ok(result)")]),
 "integer-overflow-wraps": ("exec", [(EV, "ArithOp::Add => x.checked_add(y),", "ArithOp::Add => x.checked_add(y).or(Some(x.wrapping_add(y))),")]),
 "integer-range-of-the-type-unchecked": ("exec", [(EV, "d[i] = v.filter(|v| *v >= lo && *v <= hi).ok_or_else(|| int_overflow(e, &name))?;", "d[i] = v.ok_or_else(|| int_overflow(e, &name))?;")]),
 "min-by-minus-one-allowed": ("exec", [(EV, "if y == -1 && x == lo {", "if false && y == -1 && x == lo {")]),
 "remainder-takes-the-sign-of-the-divisor": ("exec", [(EV, "x.checked_rem(y).ok_or_else(over)?", "x.checked_rem(y).map(|r| if r != 0 && ((r < 0) != (y < 0)) { r + y } else { r }).ok_or_else(over)?")]),
 "unary-minus-of-the-minimum-allowed": ("exec", [(EV, "d[i] = s[i].checked_neg().filter(|v| *v >= lo && *v <= hi).ok_or_else(|| int_overflow(e, \"a unary minus\"))?;", "d[i] = s[i].checked_neg().unwrap_or(s[i]);")]),
 "decimal-compared-by-unscaled-value": ("exec", [(EV, "cmp_decimal(x, sa, y, sb)", "x.cmp(&y)")]),
 "decimal-to-integer-truncates": ("exec", [(CA, "let v = rescale(s[i], scale, 0)", "let v = s[i].checked_div(pow10(u32::from(scale)).unwrap_or(1))")]),
 "text-to-decimal-truncates": ("exec", [(NU, "if first_dropped >= b'5' {", "if false {")]),
 "float-to-decimal-tie-rounds-to-even": ("exec", [(NU, "let tie = e + i32::from(scale) + 1 == 0;", "let tie = false;")]),
 "decimal-to-double-scale-off": ("exec", [(NU, "int_to_f64(x) / TEN[usize::from(scale)]", "int_to_f64(x) / (TEN[usize::from(scale)] * 10.0)")]),
 "cast-int-to-narrower-int-unchecked": ("exec", [(CA, "if s[i] < lo || s[i] > hi { return Err(no_fit(pos, from, to)); } d[i] = s[i];", "d[i] = s[i];")]),
 "cast-of-a-bad-date-is-null": ("exec", [(CA, "d[i] = civil::parse_date(s.get(i).trim_matches(' ')).map_err(|_| {", "d[i] = civil::parse_date(s.get(i).trim_matches(' ')).or_else(|_| Ok::<i32, ()>(0)).map_err(|_: ()| {")]),
 # ---- logic, NULL, comparison
 "null-and-false-is-null": ("exec", [(EV, "if decides(a_ok, av[i]) || decides(b_ok, bv[i]) {", "if decides(a_ok, av[i]) && decides(b_ok, bv[i]) {")]),
 "where-keeps-null-rows": ("exec", [(EV, ".filter(|r| c.valid[*r as usize] && v[*r as usize])", ".filter(|r| !c.valid[*r as usize] || v[*r as usize])")]),
 "where-or-evaluates-every-operand-on-all-rows": ("exec", [(EV, "open.retain(|r| !taken[*r as usize]);", "")]),
 "where-and-evaluates-every-operand-on-all-rows": ("exec", [(EV, "rows = self.filter_rows(p, &rows)?;", "rows = { let t = self.filter_rows(p, sel)?; rows.into_iter().filter(|r| t.contains(r)).collect() };")]),
 "and-or-in-a-projection-evaluate-every-operand": ("exec", [(EV, ".filter(|r| { let i = *r as usize; !(acc.valid[i] && v[i] != is_and) }) .collect() };", ".collect() };")]),
 "between-null-lower-bound-skips-upper-bound": ("exec", [(EV, "!(ge.valid[i] && !g[i])", "ge.valid[i] && g[i]")]),
 "between-upper-bound-evaluated-for-every-row": ("exec", [(EV, "let hi = self.eval(high, &rest)?;", "let hi = self.eval(high, sel)?;")]),
 "where-between-evaluates-the-upper-bound-for-every-row": ("exec", [(EV, "let open: Vec<u32> = sel.iter().copied().filter(|r| first_true(r) != negated).collect();", "let open: Vec<u32> = sel.to_vec();")]),
 "in-list-null-element-ignored": ("exec", [(EV, "if !it.valid[i] { null_seen = true; }", "if !it.valid[i] { }")]),
 "nan-below-everything": ("exec", [(EV, "(Comparer::F64, Data::F64(a), Data::F64(b)) => cmp_float(a[i], b[i]),", "(Comparer::F64, Data::F64(a), Data::F64(b)) => a[i].partial_cmp(&b[i]).unwrap_or(Ordering::Less),")]),
 "string-compare-case-insensitive": ("exec", [(EV, "a.get(i).as_bytes().cmp(b.get(i).as_bytes())", "a.get(i).to_lowercase().cmp(&b.get(i).to_lowercase())")]),
 "case-without-else-takes-the-first-branch": ("exec", [(EV, "remaining.retain(|r| !is_taken[*r as usize]);", "")]),
 # ---- the executor
 "project-runs-before-the-limit": ("exec", [(EX, "project_late: plan.limit.is_some(),", "project_late: false,")]),
 "filter-mask-inverted": ("exec", [(EX, "filter_record_batch(&batch, &BooleanArray::from(keep))", "filter_record_batch(&batch, &BooleanArray::from(keep.iter().map(|k| !k).collect::<Vec<bool>>()))")]),
 "deep-expressions-on-the-small-stack": ("exec", [(EX, "const DEEP: usize = 64;", "const DEEP: usize = 1_000_000_000;")]),
 "dictionary-key-off-by-one": ("exec", [(VA, "if k < values.len() { values.value(k) } else { \"\" }", "if k + 1 < values.len() { values.value(k + 1) } else { \"\" }")]),
 "scan-columns-of-a-zero-column-query-skipped": ("exec", [(EX, "options.columns = Some(vec![first_readable_column(&plan)?]);", "options.columns = Some(vec![]);")]),
 # ---- the process
 "table-numbers-left-aligned": ("cli", [(OUT, "if right { format!(\"{fill}{s}\") } else { format!(\"{s}{fill}\") }", "if false { format!(\"{fill}{s}\") } else { format!(\"{s}{fill}\") }")]),
 "data-error-exits-with-2": ("cli", [(OUT, "ErrorClass::Execution => 1,", "ErrorClass::Execution => 2,")]),
 "partial-result-not-announced": ("cli", [(OUT, "if rows_printed > 0 {", "if false && rows_printed > 0 {")]),
 "closed-stdout-is-an-error": ("cli", [(SQL, "if closed.load(Ordering::SeqCst) { return ExitCode::SUCCESS; }", "")]),
 "sigint-exits-with-1": ("cli", [(SQL, "say(&note); return ExitCode::from(130);", "say(&note); return ExitCode::from(1);")]),
 "caret-under-the-wrong-column": ("cli", [(OUT, "\" \".repeat(pre.len() + (at - from))", "\" \".repeat(pre.len() + (at - from) + 1)")]),
 "explain-analyze-counts-the-wrong-rows": ("cli", [(EXP, "ops.limit_in, ops.rows_out", "ops.rows_out, ops.limit_in")]),
 "explain-analyze-project-rows-are-the-filters-rows": ("cli", [(EXP, "ops.project_rows,", "ops.filter_rows,")]),
}


def sha_tree():
    h = hashlib.sha256()
    for crate in ("nata-exec", "nata-parquet", "nata"):
        for d, _, fs in sorted(os.walk(os.path.join(ROOT, "crates", crate))):
            if "target" in d.split(os.sep):
                continue
            for f in sorted(fs):
                if f.endswith((".rs", ".toml", ".py", ".md", ".txt")):
                    h.update(open(os.path.join(d, f), "rb").read())
    return h.hexdigest()


def stage(work):
    """A private workspace: the real root manifest, private copies of the three crates under test, symlinks to the other crates."""
    base = os.path.join(work, "ws")
    shutil.rmtree(base, ignore_errors=True)
    os.makedirs(os.path.join(base, "crates"))
    root = open(os.path.join(ROOT, "Cargo.toml")).read()
    root = re.sub(r"members = \[[^\]]*\]", 'members = ["crates/nata", "crates/nata-exec", "crates/nata-parquet"]', root)
    root = re.sub(r"exclude = \[[^\]]*\]\n", "", root)
    open(os.path.join(base, "Cargo.toml"), "w").write(root)
    shutil.copy(os.path.join(ROOT, "Cargo.lock"), os.path.join(base, "Cargo.lock"))
    for dep in ["nata-sql", "nata-scan", "nata-sched", "nata-kernels", "nata-pgwire"]:
        os.symlink(os.path.join(ROOT, "crates", dep), os.path.join(base, "crates", dep))
    for c in ["nata-exec", "nata-parquet", "nata"]:
        # copy2 would keep the old modification times, and cargo would trust the artifacts of an earlier (mutated) build
        shutil.copytree(os.path.join(ROOT, "crates", c), os.path.join(base, "crates", c),
                        ignore=shutil.ignore_patterns("target", "mutate.py", "__pycache__", "mutate*.log"), copy_function=shutil.copy)
    return base


def cargo(base, work, args, timeout=1500):
    env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(work, "target"), CARGO_TERM_COLOR="never", CARGO_NET_OFFLINE="true")
    try:
        p = subprocess.run(["cargo", "test", "--no-fail-fast"] + args, cwd=base, env=env, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return 99, "timeout"
    return p.returncode, p.stdout + p.stderr


SUITES = {"exec": [["-p", "nata-exec"]], "cli": [["-p", "nata", "--test", "sql_cli", "--test", "sql_reliability"]]}


def run_suite(suite, base, work):
    out = ""
    for args in SUITES[suite]:
        code, o = cargo(base, work, args)
        out += o
        if code != 0:
            return code, out
    return 0, out


def run_one(name, base, work):
    suite, edits = M[name]
    touched = {}
    for f, old, new in edits:
        p = os.path.join(base, f)
        touched.setdefault(f, Path(ROOT, f).read_text())
        t = Path(p).read_text()
        rx = re.compile(r"\s*".join(re.escape(c) for c in old if not c.isspace()))
        found = list(rx.finditer(t))
        if len(found) != 1:
            return "INVALID", "edit applies %d times: %s" % (len(found), old[:60])
        if re.sub(r"\s+", "", old) == re.sub(r"\s+", "", new):
            return "INVALID", "edit changes nothing"
        Path(p).write_text(t[: found[0].start()] + new + t[found[0].end():])
    code, out = run_suite(suite, base, work)
    for f, orig in touched.items():
        Path(base, f).write_text(orig)
    if code == 99:
        return "INVALID", "test suite timed out"
    if code == 0:
        return "SURVIVED", ""
    if "could not compile" in out or "error[E" in out:
        first = next((l for l in out.splitlines() if l.startswith("error")), "")
        return "INVALID", "does not compile: " + first[:100]
    failed = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)))
    if not failed:
        if "test result: FAILED" in out:
            return "CAUGHT", "a test target failed"
        if "Running " in out and ("SIGABRT" in out or "overflowed its stack" in out or "signal:" in out):
            return "CAUGHT", "the test process crashed"
        return "INVALID", "no failing test or test-process crash was recognized"
    return "CAUGHT", ", ".join(failed[:3]) + (" ..." if len(failed) > 3 else "")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--work", default=os.path.join(ROOT, "target", "mutwork-exec"))
    ap.add_argument("--out")
    a = ap.parse_args()
    if a.list:
        print("\n".join(M))
        return 0
    names = a.names or list(M)
    before = sha_tree()
    base = stage(a.work)
    t0 = time.time()
    for suite in sorted({M[n][0] for n in names}):
        code, out = run_suite(suite, base, a.work)
        if code != 0:
            print("the unmutated copy FAILS the %s suite:\n%s" % (suite, out[-3000:]))
            return 2
        print("unmutated copy: the %s suite passes (%.0fs)" % (suite, time.time() - t0), flush=True)
    rows, bad = [], 0
    for n in names:
        t = time.time()
        status, why = run_one(n, base, a.work)
        bad += status != "CAUGHT"
        rows.append((n, status, why))
        print("%-52s %-8s %s (%.0fs)" % (n, status, why, time.time() - t), flush=True)
    after = sha_tree()
    assert before == after, "the real tree changed"
    c = {s: sum(1 for r in rows if r[1] == s) for s in ("CAUGHT", "SURVIVED", "INVALID")}
    print("\n%d mutants: %d CAUGHT, %d SURVIVED, %d INVALID" % (len(rows), c["CAUGHT"], c["SURVIVED"], c["INVALID"]))
    if a.out:
        with open(a.out, "w") as f:
            f.write("| mutant | suite | result | first failing tests |\n|---|---|---|---|\n")
            for n, s, w in rows:
                f.write("| %s | %s | %s | %s |\n" % (n, M[n][0], s, w.replace("|", "/")))
            f.write("\n%d mutants: %d CAUGHT, %d SURVIVED, %d INVALID\n" % (len(rows), c["CAUGHT"], c["SURVIVED"], c["INVALID"]))
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
