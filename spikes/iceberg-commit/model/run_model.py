#!/usr/bin/env python3
"""Run the model checker on the UNMUTATED model: every baseline configuration (must PASS), the configuration that
must FAIL (I4 under snapshot isolation), the two liveness configurations, and the coverage canaries (each must be
VIOLATED, which proves the situation it names is reachable).

usage: run_model.py --jar tla2tools.jar --java java --out DIR [--work DIR] [--workers N] [--heap-mb M] [--timeout S]
                    [--budget S] [--only a,b,c]
Writes OUT/model-summary.txt (one line per run, then a total) and OUT/<run>.tlc.txt. The staged copy of the model and
TLC's state directories go to --work (default OUT/work-model) so that large state files never end up in the results.
--timeout limits one run; --budget limits the whole script (runs not started when it is spent count as mismatches).
Exit 1 on any mismatch."""
import argparse, os, re, sys, shutil, time
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import tlc_runner, mutate_model

# (name, cfg, expected): expected is "PASS" or "VIOLATED:<invariant>"
RUNS = [
 ("smoke-rest",        "smoke-rest",        "PASS"),
 ("appends2-rest",     "appends2-rest",     "PASS"),
 ("appends2-store",    "appends2-store",    "PASS"),
 ("ad-rest-ser",       "ad-rest-ser",       "PASS"),
 ("ad-store-ser",      "ad-store-ser",      "PASS"),
 ("dc-rest",           "dc-rest",           "PASS"),
 ("dc-store",          "dc-store",          "PASS"),
 ("three-rest-ser",    "three-rest-ser",    "PASS"),
 ("three-store-ser",   "three-store-ser",   "PASS"),
 ("three-rest-snap",   "three-rest-snap",   "PASS"),
 ("three-rest-snap-i4","three-rest-snap-i4","VIOLATED:I4_NoPhantom"),
 ("live-2",            "live-2",            "PASS"),
 ("live-3",            "live-3",            "PASS"),
]
CANARIES = ["Canary_Unknown", "Canary_Conflict", "Canary_RetryCommitted", "Canary_CompactCommitted",
            "Canary_DeleteCommitted", "Canary_ValidationAbort", "Canary_CrashedButCommitted",
            "Canary_LateOldAttempt", "Canary_DuplicateDelivered", "Canary_OrphanCollected", "Canary_ReaderPinned"]

def depth(path):
    m = re.search(r"depth of the complete state graph search is (\d+)", open(path, errors="replace").read())
    return m.group(1) if m else "-"

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jar", required=True)
    ap.add_argument("--java", default="java")
    ap.add_argument("--out", required=True)
    ap.add_argument("--workers", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--heap-mb", type=int, default=9000)
    ap.add_argument("--timeout", type=int, default=7200)
    ap.add_argument("--only", default="")
    ap.add_argument("--work", default="")
    ap.add_argument("--budget", type=int, default=0)
    a = ap.parse_args()
    t_start = time.time()
    def over_budget():
        return a.budget and time.time() - t_start > a.budget
    out = os.path.abspath(a.out); os.makedirs(out, exist_ok=True)
    only = set(x for x in a.only.split(",") if x)
    work = mutate_model.stage(os.path.join(os.path.abspath(a.work) if a.work else out, "work-model"))      # a fresh copy; the real model is never run in place
    real = mutate_model.sha(os.path.join(HERE, mutate_model.SPEC))
    ok, txt = tlc_runner.sany(a.java, a.jar, work)
    summary = open(os.path.join(out, "model-summary.txt"), "w")
    def say(s):
        print(s); summary.write(s + "\n"); summary.flush()
    say(f"model: IcebergCommit.tla sha256 {real}")
    if not ok:
        say("PARSE FAILED\n" + txt); return 1
    say("SANY: parses")
    bad = 0
    say(f"{'run':22} {'expected':28} {'result':10} {'detail':34} {'generated':>12} {'distinct':>11} {'depth':>5} {'secs':>7}")
    for name, cfg, expected in RUNS:
        if only and name not in only: continue
        if over_budget():
            bad += 1
            say(f"{name:22} {expected:28} {'NOT-RUN':10} the time budget of {a.budget}s is spent")
            continue
        r = tlc_runner.run_tlc(a.java, a.jar, work, f"cfg/{cfg}.cfg", os.path.join(out, f"{name}.tlc.txt"),
                               a.workers, a.timeout, a.heap_mb)
        want_status, _, want_what = expected.partition(":")
        good = (r.status == want_status) and (not want_what or r.what == want_what)
        bad += 0 if good else 1
        say(f"{name:22} {expected:28} {('OK' if good else 'MISMATCH'):10} {(r.status + ' ' + r.what)[:34]:34} {r.generated:>12} {r.distinct:>11} {depth(r.out):>5} {r.seconds:7.0f}")
    base = open(os.path.join(work, "cfg", "three-rest-ser.cfg")).read()
    for c in CANARIES:
        name = "canary-" + c.split("_", 1)[1]
        if only and name not in only: continue
        if over_budget():
            bad += 1
            say(f"{name:22} {'VIOLATED:'+c:28} {'NOT-RUN':10} the time budget of {a.budget}s is spent")
            continue
        text = re.sub(r"INVARIANTS\n(?:    \w+\n)+", f"INVARIANTS\n    {c}\n", base)
        assert text != base
        cfgname = f"cfg/{name}.cfg"
        open(os.path.join(work, cfgname), "w").write(text)
        r = tlc_runner.run_tlc(a.java, a.jar, work, cfgname, os.path.join(out, f"{name}.tlc.txt"),
                               a.workers, a.timeout, a.heap_mb)
        good = r.status == "VIOLATED" and r.what == c
        bad += 0 if good else 1
        say(f"{name:22} {'VIOLATED:'+c:28} {('REACHED' if good else 'NOT-REACHED'):10} {(r.status + ' ' + r.what)[:34]:34} {r.generated:>12} {r.distinct:>11} {depth(r.out):>5} {r.seconds:7.0f}")
    if mutate_model.sha(os.path.join(HERE, mutate_model.SPEC)) != real:
        say("FATAL: the real spec changed during the run"); return 3
    say(f"TOTAL mismatches={bad}")
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main())
