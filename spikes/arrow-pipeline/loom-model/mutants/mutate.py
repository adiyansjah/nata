#!/usr/bin/env python3
"""Mutation check of the loom model of the early-finish protocol (loom-model/tests/loom_stop.rs).

Every mutant is applied to a PRIVATE COPY (default /tmp/ap-loom-mut/work) of the files the model compiles: the spike's
own src/proto/*.rs, src/exchange.rs, src/limit.rs and the model crate. The real sources are never written; they are
hashed before and after and the run fails if they differ. A mutant is judged by loom runs of the tests of the model:

  CAUGHT    a test failed (assertion, loom deadlock, "Arc leaked", scheduler assertion) that PASSED on the unmutated copy
  SURVIVED  no test failed (a hole in the model, or an equivalent mutant)
  INVALID   the edit did not apply exactly once, or the mutant does not compile (a bug in the mutant, never a catch)

A test that timed out is not a catch. The unmutated copy must pass the same tests first ("baseline passes ..."), and
only tests that passed there can catch.

usage: mutate.py --list             one mutant name per line
       mutate.py --dry [name ...]   check that every edit applies exactly once (no cargo)
       mutate.py --check [name ...] --dry, then `cargo check --tests` of every mutant under --cfg nata_loom (compiles
                                    only, runs nothing: no mutant may be INVALID because it does not compile)
       mutate.py [name ...]         exact name first, else every mutant whose name contains the argument; none = all.
                                    The name `none` is the unmutated control: it must come out CONTROL_OK.
env:   MUT_ROOT       private root: work/ (the copy) and target/ (cargo). Default /tmp/ap-loom-mut. Concurrent runs
                      need different values. work/ is removed at the end; target/ is kept (set MUT_CLEAN=1 to remove it)
       MUT_TESTS      aimed (default): the tests aimed at each mutant plus the cheap one-worker tests;
                      all: every test of the model; quick: only the cheap tests
       MUT_ALL=1      run every test of the mode against every mutant (default: stop at the first catch)
       MUT_TEST_TIMEOUT  seconds per loom test (default 3000)
       CARGO_BUILD_JOBS (default 2), LOOM_MAX_PREEMPTIONS (passed through; unset = the default bound of each test)
exit:  0 all mutants CAUGHT, control OK, sources unchanged; 1 a survivor, an invalid mutant or a changed source;
       2 the baseline does not build or fails.
Every copy is made with FRESH mtimes (shutil.copy, never copy2): cargo decides what to rebuild from mtimes, and a
restored file that kept its old mtime would make it reuse the previous mutant's binary (mutate-reuse-check.sh).
"""
import hashlib, os, re, shutil, signal, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL = os.path.dirname(HERE)            # .../arrow-pipeline/loom-model
SPIKE = os.path.dirname(MODEL)           # .../arrow-pipeline
ROOT = os.path.abspath(os.environ.get("MUT_ROOT", "/tmp/ap-loom-mut"))
WORK = os.path.join(ROOT, "work")
TARGET = os.path.join(ROOT, "target")
TIMEOUT = int(os.environ.get("MUT_TEST_TIMEOUT", "3000"))
MODE = os.environ.get("MUT_TESTS", "aimed")
RUN_ALL = os.environ.get("MUT_ALL") == "1"

# What the model compiles, relative to the spike directory (the layout is kept: loom-model/src/lib.rs includes
# ../../src/... by #[path]).
COPY = ["loom-model/Cargo.toml", "loom-model/Cargo.lock", "loom-model/src", "loom-model/tests",
        "src/proto", "src/exchange.rs", "src/limit.rs"]
BIN_DIR = os.path.join(WORK, "loom-model")
TEST_TARGET = "loom_stop"

# The cheap tests (one worker thread, a few hundred executions): they run for every mutant and form the quick baseline.
QUICK = ["s1_one_task_limit_drops_upstream_pending_and_stops_source",
         "s2_sibling_stop_keeps_rows_the_limit_admitted_one_worker",
         "s5_task_parked_on_a_quiet_source_is_woken_by_the_stop_one_worker",
         "x3_stop_crosses_two_exchanges_one_worker"]
S2 = "s2_sibling_stop_keeps_rows_the_limit_admitted"
S3 = "s3_stop_depth_with_stages_on_both_sides_of_the_limit"
S4 = "s4_stop_of_one_pipeline_does_not_stop_the_other"
X1P = "x1_consumer_limit_stops_the_producer_producer_queued_first"
X1C = "x1_consumer_limit_stops_the_producer_consumer_queued_first"

# (name, file, old, new, [aimed tests]) -- `old` must occur exactly once in the file.
M = [
 ("stop_depth_replaced_by_pipeline_wide_flag", "src/proto/sched.rs",
  "let depth = p.stop_depth.load(SeqCst).min(self.inputs.len());",
  "let depth = self.inputs.len(); // MUTATION: a stop drops the pending input of EVERY stage, also behind the finishing one",
  [S2 + "_one_worker", S2, S3]),
 ("stop_flag_shared_by_all_pipelines", "src/proto/sched.rs",
  "            if p.stop.load(SeqCst) {",
  "            if shared.pipelines.iter().any(|q| q.stop.load(SeqCst)) { // MUTATION: one pipeline's stop stops every pipeline of the query",
  [S4, S4 + "_b_queued_first"]),
 ("stop_not_propagated_upstream_source_finish_never_called", "src/proto/sched.rs",
  "        body.source.finish();\n",
  "        // MUTATION: Source::finish() is never called, so a consumer never closes the exchange for its producers\n",
  [X1P, X1C, "x3_stop_crosses_two_exchanges_one_worker"]),
 ("stop_not_propagated_upstream_last_consumer_never_closes", "src/exchange.rs",
  "if self.consumers.fetch_sub(1, SeqCst) == 1 {",
  "if self.consumers.fetch_sub(1, SeqCst) == 99 { // MUTATION: the last consumer never closes the exchange",
  [X1P, X1C, "x3_stop_crosses_two_exchanges_one_worker"]),
 ("limit_off_by_one_finishes_one_row_early", "src/limit.rs",
  "Ok(if take == remaining { OpResult::Finished }",
  "Ok(if take + 1 >= remaining { OpResult::Finished }",
  [QUICK[0], "s1b_two_tasks_shared_limit_count_exact"]),
 ("limit_admits_one_row_too_many", "src/limit.rs",
  "Limit(Arc::new(AtomicUsize::new(n)))",
  "Limit(Arc::new(AtomicUsize::new(n + 1))) // MUTATION: LIMIT n lets n+1 rows through",
  [QUICK[0], X1P]),
 ("first_consumer_closes_the_shared_exchange", "src/exchange.rs",
  "if self.consumers.fetch_sub(1, SeqCst) == 1 {",
  "if self.consumers.fetch_sub(1, SeqCst) >= 1 { // MUTATION: the first consumer task that ends closes the exchange for the others",
  ["x2_shared_exchange_is_closed_only_by_the_last_consumer"]),
 ("wake_dropped_when_stopping", "src/proto/sched.rs",
  "    if stopped_early {\n        shared.wake_pipeline(task.pipeline);\n    }",
  "    if false { // MUTATION: the tasks parked when the pipeline stops are not woken\n        shared.wake_pipeline(task.pipeline);\n    }",
  ["s5_task_parked_on_a_quiet_source_is_woken_by_the_stop"]),
 ("pending_input_not_dropped_when_an_operator_finishes", "src/proto/sched.rs",
  "                    self.inputs[..=i].iter_mut().for_each(|x| *x = None);",
  "                    // MUTATION: the input still queued before the finishing stage is not dropped",
  [QUICK[0], "s1b_two_tasks_shared_limit_count_exact"]),
 ("stale_consumer_wakers_kept_when_the_exchange_closes", "src/exchange.rs",
  "        let stale = std::mem::take(&mut s.consumer_wakers);",
  "        let stale: Vec<Waker> = Vec::new(); // MUTATION: the stale consumer wakers (a reference cycle) stay in the channel",
  ["x4_cancel_racing_with_the_early_close", "x5_cancel_three_pipelines_one_worker"]),
 ("shutdown_does_not_release_the_tasks", "src/proto/sched.rs",
  "        lock(&self.shared.all_tasks).clear();\n",
  "        // MUTATION: wait() no longer clears all_tasks, so tasks and scheduler keep each other (and the queued morsels) alive\n",
  [QUICK[0], X1P]),
 ("stop_wakes_by_enqueueing_directly_task_runs_twice", "src/proto/sched.rs",
  "            .cloned()\n            .collect();\n        for t in tasks {\n            TaskCell::wake(&t);\n        }",
  "            .cloned()\n            .collect();\n        for t in tasks {\n            self.enqueue(t); // MUTATION: queue the task whatever its state (a running or finished task would run twice)\n        }",
  [QUICK[0], S2 + "_one_worker"]),
 ("cancel_does_not_wake_the_parked_tasks", "src/proto/sched.rs",
  'if !cfg!(feature = "bug-cancel-does-not-wake") {',
  "if false { // MUTATION: cancel() forgets to wake the parked tasks",
  ["x4_cancel_racing_with_the_early_close", "x5_cancel_three_pipelines_one_worker"]),
]


# ---------------------------------------------------------------------------------------------- helpers
def cargo_env():
    e = dict(os.environ)
    e["RUSTFLAGS"] = "--cfg nata_loom"
    e["CARGO_TARGET_DIR"] = TARGET
    e.setdefault("CARGO_BUILD_JOBS", "2")
    # The copy lives outside the repository, where rustup does not see rust-toolchain.toml, and a fresh VM has no
    # default toolchain: name the repository's channel.
    if not e.get("RUSTUP_TOOLCHAIN"):
        d = SPIKE
        while d != os.path.dirname(d):
            f = os.path.join(d, "rust-toolchain.toml")
            if os.path.isfile(f):
                m = re.search(r'^channel\s*=\s*"([^"]+)"', open(f).read(), re.M)
                if m:
                    e["RUSTUP_TOOLCHAIN"] = m.group(1)
                break
            d = os.path.dirname(d)
    return e


def sh(cmd, cwd, timeout):
    """Run in its own process group; on timeout or interruption kill the whole group (no orphaned test processes)."""
    p = subprocess.Popen(cmd, shell=True, cwd=cwd, env=cargo_env(), stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                         text=True, start_new_session=True)
    try:
        out, _ = p.communicate(timeout=timeout)
        return p.returncode, out
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, signal.SIGKILL)
        out, _ = p.communicate()
        return 124, out or ""
    except BaseException:
        os.killpg(p.pid, signal.SIGKILL)
        raise


def fresh_copy():
    # shutil.copy gives every file a fresh mtime. MUT_COPY2=1 is the KNOWN-BAD variant (shutil.copy2 keeps the old
    # mtime) that mutate-reuse-check.sh --demonstrate-failure uses to show that its check can fail; never set it otherwise.
    cp = shutil.copy2 if os.environ.get("MUT_COPY2") == "1" else shutil.copy
    shutil.rmtree(WORK, ignore_errors=True)
    for item in COPY:
        src, dst = os.path.join(SPIKE, item), os.path.join(WORK, item)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        if os.path.isdir(src):
            shutil.copytree(src, dst, copy_function=cp, ignore=shutil.ignore_patterns("__pycache__"))
        else:
            cp(src, dst)


def snapshot():
    out = {}
    for item in COPY:
        base = os.path.join(SPIKE, item)
        paths = [base]
        if os.path.isdir(base):
            paths = [os.path.join(d, f) for d, _, fs in os.walk(base) for f in fs if "__pycache__" not in d]
        for p in paths:
            out[p] = hashlib.sha256(open(p, "rb").read()).hexdigest()
    return out


def apply(name, edit):
    _, f, old, new, _ = edit
    p = os.path.join(WORK, f)
    s = open(p).read()
    n = s.count(old)
    if n != 1:
        return f"MUTATION NOT APPLIED: pattern occurs {n} times (need exactly 1) in {f}: {old[:70]!r}"
    open(p, "w").write(s.replace(old, new))
    return None


def build():
    return sh(f"cargo test --release --test {TEST_TARGET} --no-run 2>&1", BIN_DIR, 3000)


def list_tests():
    rc, out = sh(f"cargo test --release --test {TEST_TARGET} -- --list 2>/dev/null", BIN_DIR, 600)
    return re.findall(r"^(\S+): test$", out, re.M) if rc == 0 else []


def run_test(t):
    """-> (status, detail, seconds): pass | fail | timeout | error"""
    t0 = time.time()
    rc, out = sh(f"cargo test --release --test {TEST_TARGET} {t} -- --exact --nocapture 2>&1", BIN_DIR, TIMEOUT)
    dt = time.time() - t0
    if rc == 124:
        return "timeout", "", dt
    if rc == 0 and re.search(r"test result: ok\. 1 passed", out):
        line = next((l for l in out.splitlines() if l.startswith("[loom]") or l.startswith("[stress]")), "")
        return "pass", line, dt
    if rc != 0 and re.search(r"panicked at|deadlock|Arc leaked", out):
        m = re.search(r"panicked at [^\n]*\n([^\n]*)", out)
        msg = (m.group(1) if m else "").strip()[:110]
        return "fail", msg, dt
    return "error", out[-400:], dt


def selected(names):
    all_names = [m[0] for m in M]
    if not names:
        return list(M), False
    control = "none" in names
    chosen = set()
    for a in names:
        if a == "none":
            continue
        exact = [n for n in all_names if n == a]
        chosen.update(exact if exact else [n for n in all_names if a in n])
    sel = [m for m in M if m[0] in chosen]
    if not sel and not control:
        print(f"no mutant matches {names}", file=sys.stderr)
        sys.exit(2)
    return sel, control


# ---------------------------------------------------------------------------------------------- main
def main():
    args = sys.argv[1:]
    if args == ["--list"]:
        print("\n".join(m[0] for m in M))
        return 0
    check = "--check" in args
    dry = "--dry" in args or check
    names = [a for a in args if not a.startswith("--")]
    sel, control = selected(names)
    if not names:
        control = True
    before = snapshot()
    if dry:
        bad = 0
        os.makedirs(ROOT, exist_ok=True)
        for m in M:
            if m not in sel:
                continue
            fresh_copy()
            err = apply(m[0], m)
            if not err and check:
                rc, out = sh("cargo check --tests 2>&1", BIN_DIR, 3000)
                err = None if rc == 0 else "DOES NOT COMPILE\n" + out[-1500:]
            print(("BAD  " + err + "  <- ") if err else "ok   ", m[0])
            bad += bool(err)
        shutil.rmtree(WORK, ignore_errors=True)
        return 1 if bad or snapshot() != before else 0

    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    os.makedirs(ROOT, exist_ok=True)
    fresh_copy()
    print(f"private copy: {WORK}   target dir: {TARGET}   (the real sources are never written)", flush=True)
    t0 = time.time()
    rc, out = build()
    if rc:
        print("BASELINE DOES NOT BUILD:\n" + out[-2500:])
        return 2
    tests = list_tests()
    if not tests:
        print("BASELINE: no tests listed")
        return 2
    unknown = sorted({t for m in sel for t in m[4] if t not in tests} | {t for t in QUICK if t not in tests})
    if unknown:
        print(f"CONFIG ERROR: tests named in the mutant table do not exist: {unknown}")
        return 2

    def tests_for(m):
        if MODE == "all":
            return list(tests)
        if MODE == "quick":
            return list(QUICK)
        return list(dict.fromkeys(m[4] + QUICK))

    # Baseline: the unmutated copy must pass every test that is used to judge a mutant.
    if MODE == "all" or not sel:
        needed = list(tests)
    else:
        needed = list(dict.fromkeys(t for m in sel for t in tests_for(m)))
    print(f"{len(sel)} mutants; mode {MODE}; baseline tests: {len(needed)}", flush=True)
    for t in needed:
        st, detail, dt = run_test(t)
        print(f"  baseline {st:<7} {t}  {detail}  ({dt:.0f}s)", flush=True)
        if st != "pass":
            print(f"BASELINE FAILS on the unmutated copy: {t} ({st}) {detail}")
            return 2
    print(f"baseline passes ({time.time() - t0:.0f}s, {len(needed)} tests)", flush=True)

    results = []
    if control:
        results.append(("none", "CONTROL_OK", "", 0, []))
        print("CONTROL_OK  none  [the unmutated copy passed every test above]", flush=True)
    for m in sel:
        name = m[0]
        t1 = time.time()
        fresh_copy()  # fresh mtimes: cargo must rebuild
        err = apply(name, m)
        if err:
            print(f"INVALID  {name}: {err}\nMUTANT {name}: caught_by=0 missed=0 [{err}]", flush=True)
            results.append((name, "INVALID", err, 0, []))
            continue
        rc, out = build()
        if rc:
            print(f"INVALID  {name}: DOES NOT COMPILE\n{out[-1500:]}", flush=True)
            results.append((name, "INVALID", "does not compile", 0, []))
            continue
        caught, missed, slow = [], [], []
        for t in tests_for(m):
            st, detail, dt = run_test(t)
            if st == "fail":
                caught.append((t, detail))
                print(f"    {name}: {t} FAILED ({dt:.0f}s): {detail}", flush=True)
                if not RUN_ALL:
                    break
            elif st == "timeout":
                slow.append(t)
                print(f"    {name}: {t} TIMEOUT after {TIMEOUT}s (not a catch)", flush=True)
            elif st == "error":
                print(f"    {name}: {t} ERROR (not a catch): {detail}", flush=True)
                missed.append(t + "(error)")
            else:
                missed.append(t)
        verdict = "CAUGHT" if caught else "SURVIVED"
        by = ", ".join(f"{t}: {d}" for t, d in caught) or ("no test failed" + (f"; timeouts: {slow}" if slow else ""))
        print(f"{verdict:<8} {name}  [{by}]  ({time.time() - t1:.0f}s)", flush=True)
        print(f"MUTANT {name}: caught_by={len(caught)} missed={len(missed)} [missed: {' '.join(missed)}]", flush=True)
        results.append((name, verdict, by, len(caught), missed))

    shutil.rmtree(WORK, ignore_errors=True)
    if os.environ.get("MUT_CLEAN") == "1":
        shutil.rmtree(ROOT, ignore_errors=True)
    after = snapshot()
    n = lambda v: sum(r[1] == v for r in results)
    print("\n| # | mutant | result | caught by |\n|---:|---|---|---|")
    for i, (name, v, by, _, _) in enumerate(results, 1):
        print(f"| {i} | {name} | {v} | {by} |")
    print(f"\ncaught {n('CAUGHT')} of {len(sel)}; survived {n('SURVIVED')}; invalid {n('INVALID')}; control {'OK' if n('CONTROL_OK') else 'not run'}")
    unchanged = before == after
    print("real sources unchanged (hashed before and after)" if unchanged else "ERROR: the real sources changed during the run")
    ok = n("CAUGHT") == len(sel) and unchanged
    return 0 if ok else 1


sys.exit(main())
