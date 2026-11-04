#!/usr/bin/env python3
"""Checks of the QueryLifecycle model: the real configurations, the coverage canaries, and the mutants.

  check.py model   [--quick]   run every real configuration; each must PASS; each canary must be VIOLATED on its own invariant
  check.py mutants [--dry] [name ...]   each mutant edits a private copy of the spec; it must be CAUGHT (TLC reports a violation)
  check.py list
  common flags: --java J --jar JAR --out DIR --workers N --timeout S --heap-mb M
Exit status 1 on any failure (a real config that does not pass, a canary that is not violated, a mutant that survives or is invalid)."""
import argparse, hashlib, os, shutil, sys, tempfile
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
import tlc_runner
FILES = ["QueryLifecycle.tla", "MC.tla"]
REAL = ["smoke", "live-smoke", "safe-1x2", "live-safe-1x2", "safe-1x3", "live-safe-1x3", "safe-2x2", "safe-2x3", "live-safe-2x2", "live-safe-2x3"]
QUICK = ["smoke", "live-smoke", "safe-1x2", "live-safe-1x2"]
CANARIES = {"C_Succeeds": "qs=succeeded", "C_Fails": "qs=failed", "C_Zombie": "a suspected worker's result is committed",
            "C_StaleRejected": "a stale result is rejected and discarded", "C_CancelledMidRun": "cancel",
            "C_ReleasedWithFiles": "a release after a lost message"}
# name -> (what rule it breaks, [(old, new)], configs tried in order, what should catch it)
M = {
 "no-fencing": ("accept a `done` from any attempt or worker", [("Fenced(m)           == m.a = attempt[m.t] /\\ m.w = assign[m.t]", "Fenced(m)           == TRUE")], ["safe-1x2"], "I5"),
 "rerun-consumed": ("a suspected worker's already-consumed tasks are re-run and consumed again",
   [('Invalidate(t)       == tstate[t] = "running" \\/ (tstate[t] = "done" /\\ consumed[t] = 0)', 'Invalidate(t)       == tstate[t] \\in {"running", "done"}'),
    ("CanConsume(t)       == consumed[t] = 0", "CanConsume(t)       == TRUE")], ["safe-1x2"], "I1"),
 "unbounded-retries": ("no limit on attempts", [("CanRetry(t)         == attempt[t] < MaxAttempts", "CanRetry(t)         == attempt[t] < MaxAttempts + 1")], ["safe-1x3"], "I4"),
 "succeed-on-partial": ("succeed when SOME task is consumed", [("AllConsumed         == \\A t \\in Tasks : consumed[t] /= 0", "AllConsumed         == \\E t \\in Tasks : consumed[t] /= 0")], ["tiny-2x2"], "I2"),
 "commit-after-end": ("accept results after the query ended", [("AcceptOpen          == qs = \"running\"", "AcceptOpen          == TRUE")], ["tiny-2x2"], "TerminalFrozen"),
 "no-lease": ("workers never release when the coordinator dies", [("LeaseOn             == ~coordUp", "LeaseOn             == FALSE")], ["live-safe-1x2"], "L2"),
 "release-no-ack": ("treat a worker as cleaned as soon as `release` is sent", [("CleanedAfterSend(w) == {}", "CleanedAfterSend(w) == {w}")], ["live-safe-1x2"], "L2"),
 "release-skips-suspected": ("do not release suspected workers (zombies keep their files)", [("ReleaseTarget(w)    == TRUE", "ReleaseTarget(w)    == w \\notin suspect")], ["live-safe-1x2"], "L2"),
 "no-invalidate": ("suspecting a worker does not re-run its tasks", [('Invalidate(t)       == tstate[t] = "running" \\/ (tstate[t] = "done" /\\ consumed[t] = 0)', "Invalidate(t)       == FALSE")], ["live-safe-1x2"], "L1"),
 "no-resend": ("lost `start` and `done` messages are never re-sent", [("ResendOn            == TRUE", "ResendOn            == FALSE")], ["live-safe-1x2"], "L1"),
 "no-start-dedupe": ("a worker re-runs an attempt it already ran when a late `start` arrives again", [("StartFresh(m)       == Fl(m.t, m.a) \\notin ran[m.w]", "StartFresh(m)       == TRUE")], ["live-safe-1x2", "live-safe-2x2"], "L1 or L2"),
 "no-worker-left-rule": ("do not fail the query when every worker is suspected (the hang TLC found)", [("NoWorkerLeft       == suspect = Workers", "NoWorkerLeft       == FALSE")], ["live-safe-1x2"], "L1"),
}
def sha(p): return hashlib.sha256(open(p, "rb").read()).hexdigest()
def stage(dst):
    shutil.rmtree(dst, ignore_errors=True); os.makedirs(dst)
    for f in FILES: shutil.copy(os.path.join(HERE, f), os.path.join(dst, f))
    shutil.copytree(os.path.join(HERE, "cfg"), os.path.join(dst, "cfg"), copy_function=shutil.copy)
def main():
    ap = argparse.ArgumentParser(); ap.add_argument("cmd", choices=["model", "mutants", "list"]); ap.add_argument("names", nargs="*")
    ap.add_argument("--quick", action="store_true"); ap.add_argument("--dry", action="store_true")
    ap.add_argument("--java", default="java"); ap.add_argument("--jar", default="tla2tools.jar"); ap.add_argument("--out", default=os.path.join(HERE, "out"))
    ap.add_argument("--workers", type=int, default=os.cpu_count() or 4); ap.add_argument("--timeout", type=int, default=3600); ap.add_argument("--heap-mb", type=int, default=8192)
    a = ap.parse_args(); os.makedirs(a.out, exist_ok=True)
    if a.cmd == "list":
        print("\n".join(M)); return 0
    work = tempfile.mkdtemp(prefix="qlc-"); fails = 0
    def run(cfg, tag):
        return tlc_runner.run_tlc(a.java, a.jar, work, f"cfg/{cfg}.cfg", os.path.join(a.out, f"{tag}-{cfg}.txt"), a.workers, a.timeout, a.heap_mb)
    spec_hash = sha(os.path.join(HERE, "QueryLifecycle.tla"))
    if a.cmd == "model":
        stage(work); ok, txt = tlc_runner.sany(a.java, a.jar, work)
        if not ok: print("SANY FAILED\n" + txt); return 1
        for cfg in (QUICK if a.quick else REAL):
            r = run(cfg, "real"); good = r.status == "PASS"; fails += not good
            print(f"{'ok  ' if good else 'FAIL'} {cfg:16} {r.status:9} {r.distinct:>10} states {r.seconds:7.1f}s {r.what}", flush=True)
        for inv in CANARIES:
            r = run("canary-" + inv, "canary"); good = r.status == "VIOLATED" and r.what == inv; fails += not good
            print(f"{'ok  ' if good else 'FAIL'} canary {inv:20} {r.status} {r.what}", flush=True)
    else:
        names = a.names or list(M)
        stage(work); ok, txt = tlc_runner.sany(a.java, a.jar, work)       # unedited copy must parse, and pass the cheapest config
        if not ok: print("BASELINE DOES NOT PARSE"); return 1
        if not a.dry:
            r = run("smoke", "baseline")
            if r.status != "PASS": print("BASELINE FAILED", r.status, r.what); return 1
        for n in names:
            desc, edits, cfgs, expect = M[n]; stage(work); p = os.path.join(work, "QueryLifecycle.tla"); t = open(p).read(); bad = None
            for old, new in edits:
                if t.count(old) != 1: bad = f"edit applies {t.count(old)} times: {old[:50]}"; break
                t = t.replace(old, new)
            if bad: print(f"INVALID  {n}: {bad}"); fails += 1; continue
            open(p, "w").write(t); ok, txt = tlc_runner.sany(a.java, a.jar, work)
            if not ok: print(f"INVALID  {n}: mutant does not parse"); fails += 1; continue
            if a.dry: print(f"parses   {n}"); continue
            res = None
            for cfg in cfgs:
                res = run(cfg, "mut-" + n)
                if res.status != "PASS": break
            if res.status == "VIOLATED": print(f"CAUGHT   {n:26} by {res.what} on {cfg} ({expect} expected)", flush=True)
            elif res.status == "PASS": print(f"SURVIVED {n:26} (no violation on {cfgs}): the model cannot see this bug", flush=True); fails += 1
            else: print(f"INVALID  {n}: {res.status} {res.what}", flush=True); fails += 1
    if sha(os.path.join(HERE, "QueryLifecycle.tla")) != spec_hash: print("THE REAL SPEC CHANGED DURING THE RUN"); fails += 1
    shutil.rmtree(work, ignore_errors=True); return 1 if fails else 0
sys.exit(main())
