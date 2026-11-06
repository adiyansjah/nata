#!/usr/bin/env python3
"""Mutants of crates/nata-sched. Each mutant is ONE named text edit (a few have two or three) applied to a PRIVATE COPY of the crate under
target/mutants/slotN/ (never to the real sources), which is then checked by the std tests (tests/sched.rs, tests/facade.rs) and by the loom models
(tests/loom_sched.rs, RUSTFLAGS="--cfg nata_loom", one model per process, stopping at the first one that fails).

  CAUGHT    a test or a model failed, hung (timeout) or deadlocked (loom reports it)
  SURVIVED  everything passed: the tests are too weak -> fix the TESTS
  INVALID   the mutant did not compile or an anchor was not found exactly once: never counts

usage (from the repository root, under scripts/local-heavy.sh):
  python3 crates/nata-sched/mutants/mutate.py [--jobs 3] [--only REGEX] [--list] [--control] [--out FILE]
The control run (no edit) must pass everything. The copies are deleted at the end."""
import argparse, concurrent.futures as cf, os, re, shutil, subprocess, sys, time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
CRATE = os.path.join(ROOT, "crates", "nata-sched")
WORK = os.path.join(ROOT, "target", "mutants")

NOTIFY_WORK = "        self.work_cv.notify_all();\n"
M = {}

def mut(name, why, *edits):
    M[name] = (why, edits)

mut("lost-wakeup-on-window-release", "the consumer releases a window slot but does not wake the workers waiting for room",
    ("src/lib.rs", "            {\n                let mut g = lock(&sh.core);\n                g.released += 1;\n            }\n            sh.work_cv.notify_all();\n",
                   "            {\n                let mut g = lock(&sh.core);\n                g.released += 1;\n            }\n"))
mut("claim-ignores-the-stop-flag", "a worker takes a task although claiming was stopped (cancel not checked before taking a task)",
    ("src/lib.rs", "    loop {\n        if g.stop_claims {\n            return None;\n        }\n        if g.exhausted {", "    loop {\n        if g.exhausted {"))
mut("double-open-claim-before-advance", "two workers can claim the same file: the claim neither marks the opener busy nor advances the file counter (a steal race), install advances it",
    ("src/lib.rs", "                    g.next_file += 1;\n                    g.opener_busy = true;\n", ""),
    ("src/lib.rs", "    g.opener_busy = false;\n    match opened {", "    g.opener_busy = false;\n    g.next_file += 1;\n    match opened {"))
mut("two-files-opened-at-once", "the opener is not exclusive: file i+1 can be opened (and installed) before file i, the order is lost",
    ("src/lib.rs", "                    g.opener_busy = true;\n                    g.files_opened += 1;", "                    g.files_opened += 1;"))
mut("window-bound-off-by-one", "the window admits one more result than its capacity",
    ("src/lib.rs", "        if g.claimed - g.released < window {", "        if g.claimed - g.released <= window {"))
mut("error-ordered-by-arrival", "the consumer keeps the first error it sees instead of the lowest sequence number",
    ("src/lib.rs", "                    if !stopped\n                        && sink_err.is_none()\n                        && task_err.as_ref().is_none_or(|(s, _)| seq < *s)\n", "                    if !stopped && sink_err.is_none() && task_err.is_none()\n"))
mut("ordered-consumer-takes-any-slot", "the ordered consumer takes whatever finished first",
    ("src/lib.rs", "                    let pick = if ordered && emitting {", "                    let pick = if false {"))
mut("shutdown-without-joining", "the workers are detached instead of joined",
    ("src/lib.rs", "        for h in self.handles.drain(..) {\n            if !sync::join(h) {\n                all_ok = false;\n            }\n            self.joined += 1;\n        }\n",
                   "        self.handles.clear();\n"))
mut("cancel-does-not-wake-the-workers", "cancel_all does not wake workers blocked on the window or the opener",
    ("src/lib.rs", "        drop(g);\n        self.work_cv.notify_all();\n        self.out_cv.notify_all();\n    }\n}\n\n/// Cancel a running scan", "        drop(g);\n        self.out_cv.notify_all();\n    }\n}\n\n/// Cancel a running scan"))
mut("cancel-does-not-wake-the-consumer", "cancel_all does not wake a consumer that is waiting for a result",
    ("src/lib.rs", "        drop(g);\n        self.work_cv.notify_all();\n        self.out_cv.notify_all();\n    }\n}\n\n/// Cancel a running scan", "        drop(g);\n        self.work_cv.notify_all();\n    }\n}\n\n/// Cancel a running scan"))
mut("cancel-does-not-tell-running-tasks", "cancel_all stops claims but leaves abandon_from alone: running tasks never see the cancellation",
    ("src/lib.rs", "        g.stop_claims = true;\n        self.abandon_from.store(0, Ordering::SeqCst);\n        drop(g);", "        g.stop_claims = true;\n        drop(g);"))
mut("cancelled-task-not-released", "a task that was cancelled is leaked instead of dropped (its handle/buffers stay)",
    ("src/lib.rs", "                drop(task);\n                publish(&sh, seq, result);", "                if matches!(result, Run::Cancelled) {\n                    std::mem::forget(task);\n                } else {\n                    drop(task);\n                }\n                publish(&sh, seq, result);"))
mut("undelivered-results-leaked", "results nobody took are forgotten at the end instead of dropped",
    ("src/lib.rs", "        drop(leftover_slots);\n", "        std::mem::forget(leftover_slots);\n"))
mut("publish-does-not-wake-the-consumer", "a finished task does not wake the consumer",
    ("src/lib.rs", "    drop(g);\n    sh.out_cv.notify_all();\n    sh.work_cv.notify_all();\n}\n\nfn worker", "    drop(g);\n    sh.work_cv.notify_all();\n}\n\nfn worker"))
mut("exhaustion-does-not-wake-the-consumer", "the worker that finds nothing left does not tell the consumer that the claims are over",
    ("src/lib.rs", "                g.exhausted = true;\n                drop(g);\n                sh.out_cv.notify_all();\n", "                g.exhausted = true;\n                drop(g);\n"))
mut("install-does-not-wake-anyone", "the end of a file opening wakes neither the workers waiting for its tasks nor the consumer",
    ("src/lib.rs", "    drop(g);\n    sh.work_cv.notify_all();\n    sh.out_cv.notify_all();\n    drop(unused);", "    drop(g);\n    drop(unused);"))
mut("error-does-not-cancel-higher-tasks", "a failed task does not lower abandon_from: the tasks above it run on",
    ("src/lib.rs", "            g.slots.insert(seq, Slot::Failed(e));\n            g.stop_claims = true;\n            let from = sh.abandon_from.load(Ordering::SeqCst).min(seq + 1);\n            sh.abandon_from.store(from, Ordering::SeqCst);\n        }\n    }\n    drop(g);\n    sh.out_cv",
                   "            g.slots.insert(seq, Slot::Failed(e));\n            g.stop_claims = true;\n        }\n    }\n    drop(g);\n    sh.out_cv"))
mut("exit-guard-does-nothing", "a worker that panics does not tell the consumer: the consumer waits forever",
    ("src/lib.rs", "        if std::thread::panicking() {", "        if false {"))
mut("workers-not-joined-when-unwinding", "dropping the workers (the sink panicked) does not cancel and join them",
    ("src/lib.rs", "    fn drop(&mut self) {\n        let _ = self.shutdown();\n    }", "    fn drop(&mut self) {}"))
mut("facade-bypassed-by-a-std-atomic", "a std atomic is named outside src/sync.rs",
    ("src/lib.rs", "pub mod sync;\n", "pub mod sync;\n#[allow(unused_imports)]\nuse std::sync::atomic::AtomicUsize as Bypass;\n"))
mut("limit-stop-ignored", "the sink says stop and the consumer goes on",
    ("src/lib.rs", "                            Ok(Flow::Stop) => {\n                                stopped = true;\n                                emitting = false;\n                            }", "                            Ok(Flow::Stop) => {}"))
mut("open-failure-takes-no-sequence-number", "a file that cannot be opened is stored under the next number without counting it",
    ("src/lib.rs", "            let seq = g.claimed;\n            g.claimed += 1;\n            let out = g.claimed - g.released;\n            g.high_water = g.high_water.max(out);\n            g.slots.insert(seq, Slot::Failed(e));",
                   "            let seq = g.claimed;\n            let out = g.claimed - g.released;\n            g.high_water = g.high_water.max(out);\n            g.slots.insert(seq, Slot::Failed(e));"))
mut("open-failure-does-not-stop-claims", "after a file fails to open the scan goes on opening later files",
    ("src/lib.rs", "            g.slots.insert(seq, Slot::Failed(e));\n            g.stop_claims = true;\n            let from = sh.abandon_from.load(Ordering::SeqCst).min(seq + 1);\n            sh.abandon_from.store(from, Ordering::SeqCst);\n        }\n    }\n    drop(g);\n    sh.work_cv",
                   "            g.slots.insert(seq, Slot::Failed(e));\n            let from = sh.abandon_from.load(Ordering::SeqCst).min(seq + 1);\n            sh.abandon_from.store(from, Ordering::SeqCst);\n        }\n    }\n    drop(g);\n    sh.work_cv"))
# EQUIVALENT mutant, removed: "task-run-after-cancel-at-claim" (the worker no longer checks the token before calling `run`). `Work::run` is contractually
# the one that looks at the token and returns Cancelled at once, so the result is the same; the check in the worker only saves the call.

def sh(cmd, cwd, env=None, timeout=300):
    """Run in its own process group; on a timeout kill the whole group (a deadlocked test binary is a grandchild of cargo)."""
    import signal
    e = dict(os.environ); e.update(env or {})
    p = subprocess.Popen(cmd, cwd=cwd, env=e, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, start_new_session=True)
    try:
        out, _ = p.communicate(timeout=timeout)
        return p.returncode, out
    except subprocess.TimeoutExpired:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        out, _ = p.communicate()
        return 124, (out or "") + "\nTIMEOUT"

def prepare(slot, name):
    d = os.path.join(WORK, f"slot{slot}", "crate")
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    for part in ("src", "tests"):
        shutil.copytree(os.path.join(CRATE, part), os.path.join(d, part))
    shutil.copy(os.path.join(ROOT, "Cargo.lock"), d)
    with open(os.path.join(d, "Cargo.toml"), "w") as f:
        f.write('[package]\nname = "nata-sched"\nversion = "0.0.0"\nedition = "2024"\npublish = false\n\n[workspace]\n\n'
                '[target.\'cfg(nata_loom)\'.dependencies]\nloom = "0.7"\n\n[lints.rust]\nunexpected_cfgs = { level = "warn", check-cfg = [\'cfg(nata_loom)\'] }\n')
    if name is None:
        return d, None
    why, edits = M[name]
    for path, old, new in edits:
        fp = os.path.join(d, path)
        text = open(fp).read()
        if text.count(old) != 1:
            return d, f"INVALID: anchor found {text.count(old)} times in {path}: {old[:60]!r}"
        open(fp, "w").write(text.replace(old, new))
    return d, None

def models(binary, cwd, env):
    rc, out = sh([binary, "--list"], cwd, env)
    return re.findall(r"^(\S+): test$", out, re.M)

def check(slot, name):
    t0 = time.time()
    d, bad = prepare(slot, name)
    if bad:
        return name, "INVALID", bad, time.time() - t0
    base = os.path.join(WORK, f"slot{slot}")
    # 1. std tests
    env = {"CARGO_TARGET_DIR": os.path.join(base, "target-std"), "RUSTFLAGS": ""}
    rc, out = sh(["cargo", "test", "--offline", "--release", "--no-run"], d, env, 900)
    if rc != 0:
        return name, "INVALID", "does not compile (std): " + " ".join(out.strip().splitlines()[-3:]), time.time() - t0
    std = None
    for t in ("facade", "sched"):
        rc, out = sh(["cargo", "test", "--offline", "--release", "--test", t, "--", "--test-threads=4"], d, env, 60)
        if rc != 0:
            fails = re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)
            std = f"std:{t}: " + ("TIMEOUT " if rc == 124 else "") + ", ".join(fails[:2])
            break
    # 2. loom models (always run: the question is whether the MODELS catch it)
    env = {"CARGO_TARGET_DIR": os.path.join(base, "target-loom"), "RUSTFLAGS": "--cfg nata_loom"}
    rc, out = sh(["cargo", "test", "--offline", "--release", "--test", "loom_sched", "--no-run"], d, env, 900)
    if rc != 0:
        return name, "INVALID", "does not compile (loom): " + " ".join(out.strip().splitlines()[-3:]), time.time() - t0
    deps = os.path.join(base, "target-loom", "release", "deps")
    cands = sorted([os.path.join(deps, f) for f in os.listdir(deps) if f.startswith("loom_sched-") and not f.endswith(".d")], key=os.path.getmtime)
    binary = cands[-1]
    loom = None
    for m in models(binary, d, env):
        rc, out = sh([binary, m, "--exact", "--nocapture"], d, env, 150)
        if rc != 0:
            reason = "TIMEOUT" if rc == 124 else ("deadlock" if "deadlock" in out.lower() else (re.findall(r"panicked at [^\n]*\n([^\n]*)", out) or ["failed"])[0][:70])
            loom = f"loom:{m.split('_')[0]}: {reason}"
            break
    if name is None:
        if std or loom:
            return name, "CONTROL_FAILED", f"{std} | {loom}", time.time() - t0
        return name, "CONTROL_OK", "all std tests and all loom models passed", time.time() - t0
    detail = f"{loom or 'loom: no model failed'} || {std or 'std: no test failed'}"
    if loom or std:
        return name, "CAUGHT", detail + (" [loom]" if loom else " [std only]"), time.time() - t0
    return name, "SURVIVED", detail, time.time() - t0

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jobs", type=int, default=3)
    ap.add_argument("--only")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--control", action="store_true")
    ap.add_argument("--dry", action="store_true", help="only check that every anchor is found and the mutant compiles (std)")
    ap.add_argument("--out")
    a = ap.parse_args()
    if a.list:
        for n, (why, _) in M.items():
            print(f"{n}: {why}")
        return 0
    names = [n for n in M if not a.only or re.search(a.only, n)]
    jobs = [None] if a.control else names
    results = []
    try:
        with cf.ThreadPoolExecutor(a.jobs) as ex:
            futs = []
            free = list(range(a.jobs))
            import threading
            lock = threading.Lock()
            def run(n):
                with lock:
                    slot = free.pop()
                try:
                    return check(slot, n)
                finally:
                    with lock:
                        free.append(slot)
            futs = [ex.submit(run, n) for n in jobs]
            for f in cf.as_completed(futs):
                name, verdict, detail, secs = f.result()
                line = f"{(name or 'CONTROL'):<46} {verdict:<9} {detail}  ({secs:.0f}s)"
                print(line, flush=True)
                results.append(line)
    finally:
        shutil.rmtree(WORK, ignore_errors=True)
    if a.out:
        open(a.out, "w").write("\n".join(sorted(results)) + "\n")
    bad = [r for r in results if (" CAUGHT " not in r) ]
    if a.control:
        return 0 if all(" CONTROL_OK " in r for r in results) else 1
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main())
