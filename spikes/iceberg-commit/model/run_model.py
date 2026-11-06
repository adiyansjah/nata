#!/usr/bin/env python3
"""Run the model checker on the UNMUTATED model: every baseline configuration (must PASS), the configuration that
must FAIL (I4 under snapshot isolation), the two liveness configurations, and the coverage canaries (each must be
VIOLATED, which proves the situation it names is reachable).

usage: run_model.py --jar tla2tools.jar --java java --out DIR [--work DIR] [--workers N] [--heap-mb M] [--timeout S]
                    [--timeout-big S] [--big GLOBS] [--budget S] [--clamp-to-budget [--min-slice S]] [--only a,b,c] [--resume] [--list]
Writes OUT/model-summary.txt (one line per run, then a total) and OUT/<run>.tlc.txt. The staged copy of the model and
TLC's state directories go to --work (default OUT/work-model) so that large state files never end up in the results.
--timeout limits one run; --budget limits the whole script (runs not started when it is spent count as mismatches).
Targeted re-run (the `icebergmodel` GCP job): --only names the runs to do, IN THE ORDER GIVEN (an unknown name is an error, exit 2,
never a silent skip); --timeout-big replaces --timeout for the runs matching --big (comma-separated globs, default
"three-*,live-3": the three-writer runs); --clamp-to-budget cuts each run to the time left in --budget, so the whole script
cannot overrun the budget by a run's timeout; --resume reuses the row of a run that already finished with the expected
result in OUT (a restart after a preemption does not repeat it). --list prints the names of all runs and exits.
A TIMEOUT, an ERROR, a NOT-RUN or an unexpected result is a mismatch. Exit 1 on any mismatch, 2 on a usage error."""
import argparse, fnmatch, os, re, sys, shutil, time
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
 ("ddc-rest",          "ddc-rest",          "PASS"),
 ("three-rest-ser",    "three-rest-ser",    "PASS"),
 ("three-store-ser",   "three-store-ser",   "PASS"),
 ("three-rest-snap",   "three-rest-snap",   "PASS"),
 ("three-rest-snap-i4","three-rest-snap-i4","VIOLATED:I4_NoPhantom"),
 ("live-2",            "live-2",            "PASS"),
 ("live-3",            "live-3",            "PASS"),
]
BIG_DEFAULT = "three-*,live-3"
MIN_SLICE = 30   # default of --min-slice
CANARIES = ["Canary_Unknown", "Canary_Conflict", "Canary_RetryCommitted", "Canary_CompactCommitted",
            "Canary_DeleteCommitted", "Canary_ValidationAbort", "Canary_CrashedButCommitted",
            "Canary_LateOldAttempt", "Canary_DuplicateDelivered", "Canary_OrphanCollected", "Canary_ReaderPinned"]

def depth(path):
    m = re.search(r"depth of the complete state graph search is (\d+)", open(path, errors="replace").read())
    return m.group(1) if m else "-"

def all_runs():
    """Every run in the default order: (name, cfg, expected, canary-invariant or None)."""
    runs = [(n, c, e, None) for n, c, e in RUNS]
    runs += [("canary-" + c.split("_", 1)[1], "three-rest-ser", "VIOLATED:" + c, c) for c in CANARIES]
    return runs

def is_big(name, globs):
    return any(fnmatch.fnmatch(name, g) for g in globs.split(",") if g)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jar", help="tla2tools.jar")
    ap.add_argument("--java", default="java")
    ap.add_argument("--out")
    ap.add_argument("--workers", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--heap-mb", type=int, default=9000)
    ap.add_argument("--timeout", type=int, default=7200, help="seconds allowed for one run")
    ap.add_argument("--timeout-big", type=int, default=0, help="seconds allowed for a run matching --big (default: --timeout)")
    ap.add_argument("--big", default=BIG_DEFAULT, help="comma-separated globs of the runs that get --timeout-big")
    ap.add_argument("--only", default="", help="comma-separated run names, done in this order (default: all, in the default order)")
    ap.add_argument("--work", default="")
    ap.add_argument("--budget", type=int, default=0, help="seconds for the whole script (0: no limit)")
    ap.add_argument("--clamp-to-budget", action="store_true", help="cut every run to the time left in --budget")
    ap.add_argument("--min-slice", type=int, default=MIN_SLICE, help="with --clamp-to-budget, do not start a run with fewer seconds left")
    ap.add_argument("--resume", action="store_true", help="reuse the row of a run that already ended as expected in --out")
    ap.add_argument("--list", action="store_true", help="print the run names and exit")
    a = ap.parse_args()
    runs = all_runs()
    if a.list:
        print("\n".join(r[0] for r in runs)); return 0
    if not a.jar or not a.out:
        ap.error("--jar and --out are required")
    if a.clamp_to_budget and not a.budget:
        ap.error("--clamp-to-budget needs --budget")
    names = [r[0] for r in runs]
    if a.only:
        asked = list(dict.fromkeys(x.strip() for x in a.only.split(",") if x.strip()))
        unknown = [x for x in asked if x not in names]
        if unknown or not asked:
            print(f"ERROR: unknown run name(s) in --only: {', '.join(unknown) or '(none given)'}; known: {', '.join(names)}", file=sys.stderr)
            return 2
        by_name = {r[0]: r for r in runs}
        runs = [by_name[x] for x in asked]
    t_start = time.time()
    def left():
        return a.budget - (time.time() - t_start)
    out = os.path.abspath(a.out); os.makedirs(out, exist_ok=True)
    if not a.resume:
        for f in os.listdir(out):
            if f.endswith(".row"): os.remove(os.path.join(out, f))
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
    base = open(os.path.join(work, "cfg", "three-rest-ser.cfg")).read()
    for name, cfg, expected, canary in runs:
        rowfile = os.path.join(out, name + ".row")
        if a.resume and os.path.exists(rowfile):
            say(open(rowfile).read().rstrip("\n") + "  (resumed)")
            continue
        limit = a.timeout_big if (a.timeout_big and is_big(name, a.big)) else a.timeout
        why = ""
        if a.budget and left() <= 0:
            why = f"the time budget of {a.budget}s is spent"
        elif a.clamp_to_budget:
            if left() < a.min_slice:
                why = f"only {max(0, int(left()))}s of the time budget of {a.budget}s are left"
            else:
                limit = min(limit, int(left()))
        if why:
            bad += 1
            say(f"{name:22} {expected:28} {'NOT-RUN':10} {why}")
            continue
        if canary:
            text = re.sub(r"INVARIANTS\n(?:    \w+\n)+", f"INVARIANTS\n    {canary}\n", base)
            assert text != base
            cfgname = f"cfg/{name}.cfg"
            open(os.path.join(work, cfgname), "w").write(text)
        else:
            cfgname = f"cfg/{cfg}.cfg"
        r = tlc_runner.run_tlc(a.java, a.jar, work, cfgname, os.path.join(out, f"{name}.tlc.txt"),
                               a.workers, limit, a.heap_mb)
        want_status, _, want_what = expected.partition(":")
        good = (r.status == want_status) and (not want_what or r.what == want_what)
        bad += 0 if good else 1
        label = ("OK" if good else "MISMATCH") if not canary else ("REACHED" if good else "NOT-REACHED")
        row = f"{name:22} {expected:28} {label:10} {(r.status + ' ' + r.what)[:34]:34} {r.generated:>12} {r.distinct:>11} {depth(r.out):>5} {r.seconds:7.0f}"
        say(row)
        if good:
            open(rowfile, "w").write(row + "\n")
    if mutate_model.sha(os.path.join(HERE, mutate_model.SPEC)) != real:
        say("FATAL: the real spec changed during the run"); return 3
    say(f"TOTAL mismatches={bad}")
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main())
