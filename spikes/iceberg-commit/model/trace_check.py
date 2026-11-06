#!/usr/bin/env python3
"""Trace validation of the Rust writers against the TLA+ model (verification-plan item 24, the code gate).

For every trace in a directory (*.jsonl written by the suite under IC_TRACE=<dir>): translate it (trace_to_tla.py) into a
run of TraceSpec.tla and let TLC decide whether the model allows that behaviour.
  VALID     TLC reached the end of the trace: the invariant NotDone is violated, and no model invariant was violated on the way
  INVALID   TLC deadlocked before the end (the reason names the first event the model does not allow, with its index and the
            writer), or a model invariant (I1..I7) was violated by the replayed behaviour
  UNMODELLED the trace holds something the model has no action for (a rejected commit, another catalog error ...): no verdict
  ERROR     anything else (TLC did not run, did not finish, printed something unrecognised, a malformed trace)
Only VALID and INVALID are verdicts; UNMODELLED and ERROR are never evidence.

Each trace starts with a `meta` event written by the suite: the writer mode and what the suite expects of that run.
  expect "pass"          (the correct writer, or a known-bad writer in a scenario where it must not misbehave): must be VALID
  expect "detect"        (a known-bad writer in the scenario that exposes it): must be INVALID
  expect "informational" (stress with known-bad writers: a race may or may not happen): reported, not judged
A run is judged by that expectation (OK / MISMATCH). With --all-valid the verdict is instead: every trace must be VALID.

usage: trace_check.py --jar tla2tools.jar --java java --traces DIR [--out DIR] [--work DIR] [--jobs N] [--timeout S]
                      [--heap-mb M] [--only SUBSTRING] [--all-valid] [--model-dir DIR]
Writes OUT/trace-summary.txt and OUT/<trace>.tlc.txt. Exit 0: every trace judged OK; 1: a MISMATCH, UNMODELLED or ERROR; 2: usage."""
import argparse, concurrent.futures as cf, glob, json, os, re, shutil, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

RE_STATE_I = re.compile(r"^/\\ i = (\d+)\s*$", re.M)
RE_INV = re.compile(r"Error: Invariant (\S+) is violated")

def classify(text, rc):
    """-> (status, detail, i). status: VALID | INVALID | ERROR; i: the value of the index in the last state printed (or 0)."""
    states = [int(x) for x in RE_STATE_I.findall(text)]
    last = states[-1] if states else 0
    m = RE_INV.search(text)
    if m:
        if m.group(1) == "NotDone":
            return "VALID", "", last
        return "INVALID", f"invariant {m.group(1)} violated", last
    if "Error: Deadlock reached" in text:
        return "INVALID", "deadlock", last
    m = re.search(r"^(Error: .*|.*Exception.*|.*OutOfMemory.*)$", text, re.M)
    if "Model checking completed. No error has been found." in text:
        return "ERROR", "TLC finished without reaching the end of the trace and without a deadlock", last
    return "ERROR", (m.group(1)[:200] if m else f"exit status {rc}, no recognised TLC output"), last

def describe(ev):
    """One line for an abstract event, for the reason of an INVALID verdict."""
    keys = [k for k in ev if k not in ("a", "w")]
    return f"{ev['a']} by model writer {ev['w']}" + (" (" + ", ".join(f"{k}={ev[k]}" for k in keys) + ")" if keys else "")

def judge(meta, status, all_valid):
    if all_valid:
        return "OK" if status == "VALID" else "MISMATCH"
    expect = meta.get("expect", "informational")
    if expect == "informational":
        return "INFO"
    if status not in ("VALID", "INVALID"):
        return "MISMATCH"
    return "OK" if (expect == "pass") == (status == "VALID") else "MISMATCH"

def run_one(path, a, tt):
    name = os.path.basename(path)[:-len(".jsonl")]
    row = dict(name=name, status="ERROR", detail="", meta={}, events=0, secs=0.0)
    t0 = time.time()
    try:
        t = tt.translate(path)
    except tt.Unmodelled as e:
        row.update(status="UNMODELLED", detail=str(e))
        return row
    except tt.BadTrace as e:
        row.update(status="ERROR", detail=f"malformed trace: {e}")
        return row
    except Exception as e:                                   # a translator bug must not take the other traces down
        row.update(status="ERROR", detail=f"the translator failed: {type(e).__name__}: {e}")
        return row
    row["meta"], row["events"] = t["meta"], len(t["trace"])
    work = os.path.join(a.work, name)
    shutil.rmtree(work, ignore_errors=True)
    tt.render(t, work)
    for f in ("IcebergCommit.tla", "TraceSpec.tla"):
        shutil.copy(os.path.join(a.model_dir, f), os.path.join(work, f))          # fresh copies, fresh mtimes
    out = os.path.join(a.out, name + ".tlc.txt")
    # TLC unpacks its standard modules (TLC.tla ...) into java.io.tmpdir: one directory per run, or parallel runs race on /tmp/TLC.tla
    os.makedirs(os.path.join(work, "tmp"), exist_ok=True)
    cmd = [a.java, "-XX:+UseParallelGC", f"-Xmx{a.heap_mb}m", f"-Djava.io.tmpdir={os.path.join(work, 'tmp')}", "-cp", a.jar, "tlc2.TLC", "-config", "Tr.cfg", "-workers", "1",
           "-metadir", "meta", "-cleanup", "Tr"]
    try:
        p = subprocess.run(cmd, cwd=work, capture_output=True, text=True, timeout=a.timeout, errors="replace")
        text, rc = p.stdout + p.stderr, p.returncode
    except subprocess.TimeoutExpired:
        row.update(status="ERROR", detail=f"TLC did not finish within {a.timeout}s", secs=time.time() - t0)
        return row
    open(out, "w").write("$ " + " ".join(cmd) + "\n" + text)
    status, detail, i = classify(text, rc)
    if status == "INVALID":
        ev = t["trace"][i - 1] if 1 <= i <= len(t["trace"]) else None
        w = t["writers"].get(ev["w"], "?") if ev else "?"
        what = f"event {i} of {len(t['trace'])}: {describe(ev)} [{w}]" if ev else f"after event {i}"
        row["detail"] = (f"the model does not allow {what}" if detail == "deadlock" else f"{detail} {what}")
    else:
        row["detail"] = detail
    row.update(status=status, secs=time.time() - t0)
    shutil.rmtree(work, ignore_errors=True)
    return row

def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--jar"); ap.add_argument("--java", default="java"); ap.add_argument("--traces"); ap.add_argument("--out")
    ap.add_argument("--work", default=""); ap.add_argument("--jobs", type=int, default=min(4, os.cpu_count() or 2))
    ap.add_argument("--timeout", type=int, default=300); ap.add_argument("--heap-mb", type=int, default=1024)
    ap.add_argument("--only", default=""); ap.add_argument("--all-valid", action="store_true")
    ap.add_argument("--model-dir", default=HERE, help="where IcebergCommit.tla and TraceSpec.tla are (default: next to this script)")
    a = ap.parse_args(argv)
    if not (a.jar and a.traces and a.out):
        ap.error("--jar, --traces and --out are required")
    import trace_to_tla as tt
    a.out = os.path.abspath(a.out); a.model_dir = os.path.abspath(a.model_dir)
    a.work = os.path.abspath(a.work or os.path.join(a.out, "work-trace"))
    os.makedirs(a.out, exist_ok=True); os.makedirs(a.work, exist_ok=True)
    paths = sorted(p for p in glob.glob(os.path.join(a.traces, "*.jsonl")) if a.only in os.path.basename(p))
    if not paths:
        print(f"ERROR: no traces matching {a.only!r} in {a.traces}", file=sys.stderr)
        return 2
    with cf.ThreadPoolExecutor(max_workers=a.jobs) as ex:
        rows = list(ex.map(lambda p: run_one(p, a, tt), paths))
    lines, bad = [], 0
    counts = {}
    for r in rows:
        j = judge(r["meta"], r["status"], a.all_valid)
        bad += j == "MISMATCH" or r["status"] in ("ERROR", "UNMODELLED")
        key = (r["meta"].get("mode", "?"), r["meta"].get("expect", "?"), r["status"])
        counts[key] = counts.get(key, 0) + 1
        lines.append(f"{r['name']:<58} {r['meta'].get('mode', '?'):<19} expect {r['meta'].get('expect', '?'):<13} {r['status']:<10} {j:<8} "
                     f"{r['events']:>4} events {r['secs']:5.1f}s  {r['detail']}")
    lines.append("")
    for (mode, expect, status), n in sorted(counts.items()):
        lines.append(f"{n:>4} x mode {mode:<19} expect {expect:<13} -> {status}")
    lines.append(f"TOTAL traces={len(rows)} valid={sum(r['status'] == 'VALID' for r in rows)} invalid={sum(r['status'] == 'INVALID' for r in rows)} "
                 f"unmodelled={sum(r['status'] == 'UNMODELLED' for r in rows)} error={sum(r['status'] == 'ERROR' for r in rows)} mismatches={bad}")
    text = "\n".join(lines)
    print(text)
    json.dump([dict(r, judged=judge(r["meta"], r["status"], a.all_valid)) for r in rows], open(os.path.join(a.out, "trace-results.json"), "w"), indent=1)
    open(os.path.join(a.out, "trace-summary.txt"), "w").write(text + "\n")
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main())
