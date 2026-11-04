#!/usr/bin/env python3
"""Mutation check of the arrow-pipeline tests.

Every mutant is applied to a PRIVATE COPY of the spike (default /tmp/ap-mut/work) -- never to the real sources --
and the test suite (then the DuckDB diff and nullcheck, if the tests pass) must fail. A mutant that survives
everything is printed as SURVIVED and the script exits 1.

Usage: scripts/mutate.py [name ...]   (no argument: all mutants)
       scripts/mutate.py --list       print one exact mutant name per line and exit
An argument selects the mutant whose name equals it exactly; if no mutant has exactly that name it selects every
mutant whose name contains it. (Sharding: run `--list`, give each shard its own names and its own MUT_ROOT.)
Env: MUT_ROOT (default /tmp/ap-mut) is the ONLY root of everything this script creates: the private copy
     ($MUT_ROOT/work) and the cargo target dir ($MUT_ROOT/target). Several runs side by side need different MUT_ROOT
     values and nothing else (no fixed path, port or file elsewhere in the spike).
     CARGO_BUILD_JOBS (default 2), MUT_TIMEOUT secs per cargo test (default 900)
     MUT_KEEP=1 keeps $MUT_ROOT after the run (default: it is deleted on exit, also on SIGTERM / Ctrl-C, so a run
     never leaves a copy or a target dir behind; /tmp is often a small RAM-backed tmpfs). The build needs about
     2 GB under $MUT_ROOT; pick a MUT_ROOT on a real disk if /tmp is small.
"""
import os, re, shutil, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.dirname(HERE)
ROOT = os.environ.get("MUT_ROOT", "/tmp/ap-mut")
WORK = os.path.join(ROOT, "work")
TARGET = os.path.join(ROOT, "target")
LINEITEM = os.path.realpath(os.environ.get("LINEITEM", os.path.join(SRC, "../../data/tpch-sf1/lineitem.parquet")))
ENV = dict(os.environ, CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "2"), CARGO_TARGET_DIR=TARGET,
           WATCHDOG_SECS="6", LIMIT_CASES="800", STRESS_REPS="150", STRESS_RERUNS="2", LINEITEM=LINEITEM)
TIMEOUT = int(os.environ.get("MUT_TIMEOUT", "900"))

# (name, file, old, new)   `old` must occur exactly once in the file
M = [
 ("filter(date) ignores the incoming selection", "src/ops.rs",
  "let (c, v) = input.date(self.0.col);\n            filter_range(c, v, self.0.lo, self.0.hi, input.rows(), input.sel())",
  "let (c, v) = input.date(self.0.col);\n            filter_range(c, v, self.0.lo, self.0.hi, input.rows(), None)"),
 ("filter(decimal) ignores the incoming selection", "src/ops.rs",
  "let (c, v) = input.dec(&Ref::Col(self.0.col));\n            filter_range(c, v, self.0.lo, self.0.hi, input.rows(), input.sel())",
  "let (c, v) = input.dec(&Ref::Col(self.0.col));\n            filter_range(c, v, self.0.lo, self.0.hi, input.rows(), None)"),
 ("LIMIT counts physical rows before the selection", "src/ops.rs",
  "let take = remaining.min(input.len());\n            if self.0.compare_exchange", "let take = remaining.min(input.rows());\n            if self.0.compare_exchange"),
 ("off-by-one in LIMIT (Finished one row early)", "src/ops.rs",
  "Ok(if take == remaining { OpResult::Finished }", "Ok(if take + 1 >= remaining { OpResult::Finished }"),
 ("LIMIT counter is not atomic (store instead of compare-exchange)", "src/ops.rs",
  "if self.0.compare_exchange(remaining, remaining - take, SeqCst, SeqCst).is_ok() {", "if { self.0.store(remaining - take, SeqCst); true } {"),
 ("early finish by an operator not propagated (task keeps reading the source)", "src/proto/sched.rs",
  "                    self.finishing = true;\n                    self.finish_depth = self.finish_depth.max(i + 1);\n", ""),
 ("early finish by the sink not propagated", "src/proto/sched.rs",
  "                        self.finishing = true;\n                        self.finish_depth = self.finish_depth.max(n + 1);\n", ""),
 ("stop flag never published to the other tasks (sideways)", "src/proto/sched.rs",
  "                    p.stop.store(true, SeqCst);\n                    return Ok(Step::Done);", "                    return Ok(Step::Done);"),
 ("wake lost on early finish (parked tasks not woken)", "src/proto/sched.rs",
  "    if stopped_early {\n        shared.wake_pipeline(task.pipeline);\n    }", "    if false {\n        shared.wake_pipeline(task.pipeline);\n    }"),
 ("cancel does not wake parked tasks", "src/proto/sched.rs", 'if !cfg!(feature = "bug-cancel-does-not-wake") {', "if false {"),
 ("wake that arrives while the task runs is forgotten", "src/proto/sched.rs", 'if !cfg!(feature = "bug-wake-ignored-while-running") {', "if false {"),
 ("Source::finish() never called", "src/proto/sched.rs", "        body.source.finish();\n", ""),
 ("sideways stop drops batches already admitted by LIMIT (the bug found by this spike)", "src/proto/sched.rs",
  "let depth = p.stop_depth.load(SeqCst).min(self.inputs.len());", "let depth = self.inputs.len();"),
 ("sink combine overwrites instead of merging (loses other tasks' partial state)", "src/sinks.rs",
  "Some(t) => t.merge(local),", "Some(t) => *t = local,"),
 ("collect combine keeps only the first batch of each task", "src/sinks.rs",
  "lock(&self.global.out).extend(self.mine);", "lock(&self.global.out).extend(self.mine.into_iter().take(1));"),
 ("Q6 sink loses the non-NULL count", "src/sinks.rs", "self.acc.nn += n;", "self.acc.nn += 0;"),
 ("LIMIT sink keeps the whole batch instead of cutting it", "src/sinks.rs",
  "self.mine.push(truncate(input, take));", "self.mine.push(std::mem::take(input));"),
 ("blocked sink consumes its input", "src/sinks.rs",
  "                return Ok(SinkResult::Blocked);", "                std::mem::take(input);\n                return Ok(SinkResult::Blocked);"),
 ("NULL handling in the sum: NULL values are added", "src/genops/kernels.rs",
  "s += (v[i] as i128) * (ok as i128);", "s += v[i] as i128;"),
 ("filter keeps NULL rows (dense/masked path)", "src/genops/kernels.rs",
  "k += ((x >= lo) & (x <= hi) & valid.ok(i)) as usize;", "k += ((x >= lo) & (x <= hi)) as usize;"),
 ("filter lower bound off by one", "src/genops/kernels.rs",
  "k += ((x >= lo) & (x <= hi)) as usize;", "k += ((x > lo) & (x <= hi)) as usize;"),
 ("masked grouped sums miscount NULLs", "src/genops/kernels.rs", "nulls[j][g] += !ok as u64;", "nulls[j][g] += 0;"),
 ("grouped count wrong (dense path)", "src/genops/kernels.rs",
  "count[g] += 1;\n        for j in 0..K {\n            acc[g][j] += cols[j][i] as i128;", "count[g] += 2;\n        for j in 0..K {\n            acc[g][j] += cols[j][i] as i128;"),
 ("project ignores the selection (errors raised for removed rows)", "src/ops.rs",
  "StepSpec::Mul { a, b } => q(mul(cur.dec(a), cur.dec(b), cur.rows(), cur.sel()))?,", "StepSpec::Mul { a, b } => q(mul(cur.dec(a), cur.dec(b), cur.rows(), None))?,"),
 ("with_sel drops the computed columns", "src/batch.rs",
  "Batch { data: self.data.clone(), sel: Some(Arc::new(sel)), tmp: self.tmp.clone() }", "Batch { data: self.data.clone(), sel: Some(Arc::new(sel)), tmp: Vec::new() }"),
 ("Batch::len ignores the selection", "src/batch.rs", "Some(s) => s.len(),\n            None => self.rows(),", "Some(_) => self.rows(),\n            None => self.rows(),"),
 ("parquet scan skips the last row group", "src/sources.rs",
  "if m >= self.scan.file.row_groups {", "if m + 1 >= self.scan.file.row_groups {"),
 ("memory scan skips the last morsel", "src/sources.rs",
  "if m >= self.scan.morsels.len() {", "if m + 1 >= self.scan.morsels.len() {"),
 ("exchange producer does not compact (selection vector crosses the exchange)", "src/exchange.rs",
  "self.pending = Some(input.compact().map_err(|e| QueryError(format!(\"compact: {e}\")))?);", "self.pending = Some(input.clone());"),
 ("exchange consumer never closes the channel (early finish does not cross)", "src/exchange.rs",
  "        if self.consumers.fetch_sub(1, SeqCst) == 1 {\n            self.chan.close_consumers();\n        }", "        self.consumers.fetch_sub(1, SeqCst);"),
 ("exchange pop does not wake blocked producers", "src/exchange.rs",
  "            let to_wake = std::mem::take(&mut s.producer_wakers);\n            drop(s);\n            wake_all(to_wake);\n            return SourceResult::Batch(b);",
  "            drop(s);\n            return SourceResult::Batch(b);"),
 ("overflow-checks switched off in the profile", "Cargo.toml", "overflow-checks = true", "overflow-checks = false"),
 ("source claims counter not incremented (early-finish counter blind)", "src/sources.rs",
  "                self.scan.stats.claimed.fetch_add(1, SeqCst);\n                self.cur = Some((m, 0));", "                self.cur = Some((m, 0));"),
]

def sh(cmd, timeout, env=ENV, cwd=WORK):
    """Run in its own process group; on timeout kill the whole group (no orphaned test processes)."""
    import signal
    p = subprocess.Popen(cmd, shell=True, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, start_new_session=True)
    try:
        out, _ = p.communicate(timeout=timeout)
        return p.returncode, out
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, signal.SIGKILL)
        out, _ = p.communicate()
        return 124, out or ""
    except BaseException:  # interrupted (SIGTERM / Ctrl-C): do not leave cargo or test processes behind
        os.killpg(p.pid, signal.SIGKILL)
        raise

def failed_tests(out):
    names = re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)
    return names

def suite():
    """Returns (caught_by or None, detail)."""
    rc, out = sh("cargo test --profile mutate --no-fail-fast 2>&1", TIMEOUT)
    if "No space left on device" in out:
        return "INFRA ERROR (disk full: put MUT_ROOT on a bigger disk, /tmp may be a small tmpfs)", out[-600:]
    if "error[E" in out or "could not compile" in out:
        return "COMPILE ERROR (mutant invalid)", out[-1500:]
    names = failed_tests(out)
    if rc == 124:
        return "tests (timeout)", out[-300:]
    if names:
        return "tests: " + ", ".join(sorted(set(n.split("::")[-1] for n in names))), ""
    if rc != 0:
        return "tests (nonzero exit)", out[-500:]
    rc, out = sh(f"{TARGET}/mutate/spike-arrow-pipeline nullcheck {LINEITEM}", 300)
    if rc != 0:
        return "nullcheck", out[-300:]
    env = dict(ENV, BIN=f"{TARGET}/mutate/spike-arrow-pipeline")
    rc, out = sh(f"bash scripts/duckdb_diff.sh {LINEITEM}", 300, env)
    if rc != 0:
        return "duckdb_diff", out[-300:]
    return None, ""

def selected(only):
    """Exact name first, substring second, per argument. Order of M is kept."""
    names = [m[0] for m in M]
    chosen = set()
    for a in only:
        exact = [n for n in names if n == a]
        chosen.update(exact if exact else [n for n in names if a in n])
    return chosen

def cleanup():
    if os.environ.get("MUT_KEEP") != "1" and os.path.isdir(ROOT):
        shutil.rmtree(ROOT, ignore_errors=True)
        print(f"removed {ROOT}", flush=True)

def main():
    only = sys.argv[1:]
    if only == ["--list"]:
        print("\n".join(m[0] for m in M))
        return
    chosen = selected(only) if only else None
    if only and not chosen:
        print("no mutant matches", only)
        sys.exit(2)
    import signal
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))  # run the `finally` below on SIGTERM too
    if os.path.exists(WORK):
        shutil.rmtree(WORK)
    os.makedirs(ROOT, exist_ok=True)
    free = shutil.disk_usage(ROOT).free
    if free < 4 * 2**30:
        print(f"WARNING: only {free / 2**30:.1f} GiB free under {ROOT}; the build needs about 2 GiB (set MUT_ROOT to a real disk)", flush=True)
    try:
        shutil.copytree(SRC, WORK, ignore=shutil.ignore_patterns("target", ".git", "out"), copy_function=shutil.copyfile)  # fresh mtimes: cargo must rebuild
        print(f"private copy: {WORK}   target dir: {TARGET}   (the real sources are never touched)", flush=True)
        t0 = time.time()
        c, d = suite()
        if c is not None:
            print("BASELINE FAILS in the private copy:", c, d)
            sys.exit(2)
        print(f"baseline passes ({time.time()-t0:.0f}s)", flush=True)
        results = []
        for i, (name, f, old, new) in enumerate(M, 1):
            if chosen is not None and name not in chosen:
                continue
            path = os.path.join(WORK, f)
            orig = open(path).read()
            n = orig.count(old)
            if n != 1:
                print(f"SKIP (pattern occurs {n} times, need 1): {name}")
                results.append((name, "PATTERN", ""))
                continue
            open(path, "w").write(orig.replace(old, new))
            t = time.time()
            try:
                caught, detail = suite()
            finally:
                open(path, "w").write(orig)
            verdict = caught if caught else "SURVIVED"
            bad_run = bool(caught) and caught.startswith(("COMPILE", "INFRA"))
            label = "NOT A RESULT" if bad_run else ("CAUGHT by " + caught if caught else "SURVIVED")
            print(f"[{i:2d}/{len(M)}] {label}{(' -- ' + caught) if bad_run else ''}: {name}   ({time.time()-t:.0f}s)", flush=True)
            if bad_run:
                print(detail)
            results.append((name, verdict, detail))
        bad = [r for r in results if r[1] in ("SURVIVED", "PATTERN") or r[1].startswith(("COMPILE", "INFRA"))]
        ok = len(results) - len(bad)
        print(f"\n{ok} of {len(results)} mutants caught; {len(bad)} problem(s)")
        print("\n| # | mutant | caught by |\n|---:|---|---|")
        for i, (name, v, _) in enumerate(results, 1):
            print(f"| {i} | {name} | {v} |")

    finally:
        cleanup()
    sys.exit(1 if bad else 0)

main()
