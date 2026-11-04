#!/usr/bin/env python3
"""Mutation check of the Iceberg commit model.

Each mutant is a small text edit of model/IcebergCommit.tla, applied to a PRIVATE COPY (fresh files made with
shutil.copy, so fresh mtimes: TLC and SANY never see a stale timestamp); the real spec is hashed before and after
and must be byte-identical. A mutant is
  CAUGHT    TLC reports a violated invariant on one of the mutant's configurations (the invariant is named);
  SURVIVED  TLC passes on every configuration it was tried on (the model cannot see this bug: a failure);
  INVALID   an edit does not apply exactly once, the copy does not parse, or TLC fails for another reason
            (a bug in the mutant or the model, never evidence; a failure).
The unmutated baseline is run FIRST on the same staging code (an unedited copy must parse, and, with --baseline,
pass on the cheapest configuration), so a broken harness cannot produce a CAUGHT.

usage: mutate_model.py [--list] [--dry] [--baseline] [--java J] [--jar JAR] [--out DIR] [--work DIR] [--workers N]
                       [--timeout S] [--heap-mb M] [name ...]
  --list       print the mutant names
  --dry        stage every selected mutant, check every edit applies once, and (with --jar) parse it with SANY; no TLC
  --baseline   also run TLC on the unedited copy first (config smoke-rest) and require PASS
Exit status 1 if any mutant SURVIVED or was INVALID, or the baseline failed.
"""
import argparse, hashlib, os, shutil, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import tlc_runner

SPEC = "IcebergCommit.tla"
FILES = ["IcebergCommit.tla", "MC.tla"]

# name -> (description, [(old, new)], configurations tried in order, invariants expected to catch it)
MUTANTS = {
 "no-cas": ("remove the compare-and-swap: a commit is accepted whatever version the writer based it on (blind overwrite)",
    [("CasOk(q) == CurVer = q.exp", "CasOk(q) == TRUE")],
    ["appends2-rest", "appends2-store", "three-rest-ser"], ["I2_NoLostUpdate", "I6_SeqIncreases", "I7_NoSnapshotLost"]),
 "skip-validation": ("skip the conflict validation on retry (compaction resurrects deleted rows; delete misses a concurrent insert)",
    [("Validated(w) == Valid(w, CurMeta)", "Validated(w) == TRUE")],
    ["dc-rest", "ad-rest-ser", "three-rest-ser"], ["I2_NoLostUpdate", "I4_NoPhantom"]),
 "blind-retry-after-unknown": ("treat CommitStateUnknown as failure and retry without looking for its own snapshot",
    [('Mine(w) == \\E s \\in CurMeta.snaps : s.w = w',
      'Mine(w) == pc[w] # "unknown" /\\ \\E s \\in CurMeta.snaps : s.w = w')],
    ["smoke-rest", "appends2-rest", "appends2-store"], ["I5_AtMostOnce"]),
 "commit-before-durable": ("send the commit before the data file upload has completed",
    [('UploadDone(w) == pc[w] = "uploaded"', 'UploadDone(w) == pc[w] \\in {"uploading", "uploaded"}')],
    ["smoke-rest", "appends2-rest"], ["I1_FilesExist", "I3_ReaderSeesSnapshot"]),
 "reuse-sequence-number": ("a new snapshot gets the previous sequence number instead of the next one",
    [("NextSeq(bm) == bm.lastSeq + 1", "NextSeq(bm) == bm.lastSeq")],
    ["smoke-rest", "appends2-rest"], ["I6_SeqIncreases"]),
 "compaction-deletes-live-files": ("compaction physically deletes its input files when the commit is acknowledged (older snapshots still reference them)",
    [("PurgeOnAck(w) == {}", 'PurgeOnAck(w) == IF OpOf[w] = "compact" THEN plan[w].inputs ELSE {}')],
    ["dc-rest", "three-rest-ser"], ["I1_FilesExist", "I3_ReaderSeesSnapshot"]),
 "accept-stale-base-on-retry": ("on retry, refresh only the expected version and keep building on the stale base",
    [("NewBaseOnRetry(w) == CurVer", "NewBaseOnRetry(w) == base[w]")],
    ["appends2-rest", "appends2-store", "three-rest-ser"], ["I2_NoLostUpdate", "I6_SeqIncreases", "I7_NoSnapshotLost"]),
 "gc-ignores-active-writers": ("orphan collection also deletes files of writers that are still running",
    [("GCOwnerDead(f) == pc[Owner(f)] \\in Terminal", "GCOwnerDead(f) == TRUE")],
    ["smoke-rest", "appends2-rest"], ["I1_FilesExist"]),
 "gc-ignores-inflight-requests": ("orphan collection deletes files that a request still in the network refers to",
    [("GCNoRequest(f) == \\A q \\in net : f \\notin Refs(q.snap)", "GCNoRequest(f) == TRUE")],
    ["smoke-rest", "appends2-rest"], ["I1_FilesExist"]),
}
REQUIRED = ["no-cas", "skip-validation", "blind-retry-after-unknown", "commit-before-durable",
            "reuse-sequence-number", "compaction-deletes-live-files", "accept-stale-base-on-retry"]

def sha(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()

def stage(dst):
    """Fresh copy of the model into dst (which is created empty). shutil.copy gives every file a fresh mtime."""
    if os.path.exists(dst):
        shutil.rmtree(dst)
    os.makedirs(dst)
    for f in FILES:
        shutil.copy(os.path.join(HERE, f), os.path.join(dst, f))
    shutil.copytree(os.path.join(HERE, "cfg"), os.path.join(dst, "cfg"), copy_function=shutil.copy)
    return dst

def apply_edits(path, edits):
    """Apply the edits to the copy at path. Returns None, or a message when an edit does not apply exactly once."""
    text = open(path).read()
    for old, new in edits:
        n = text.count(old)
        if n != 1:
            return f"edit applies {n} times (must be exactly once): {old!r}"
        text = text.replace(old, new)
    open(path, "w").write(text)
    return None

def make_mutant(name, root, edits=None):
    """Stage and edit one mutant under root/name. Returns (dir, error or None)."""
    d = stage(os.path.join(root, name))
    err = apply_edits(os.path.join(d, SPEC), edits if edits is not None else MUTANTS[name][1])
    return d, err

def main():
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("names", nargs="*")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--dry", action="store_true")
    ap.add_argument("--baseline", action="store_true")
    ap.add_argument("--java", default=os.environ.get("JAVA", "java"))
    ap.add_argument("--jar", default=os.environ.get("TLA2TOOLS", ""))
    ap.add_argument("--out", default=os.path.join(HERE, "work-mutants"))
    ap.add_argument("--work", default="", help="where mutant copies and TLC state directories go (default: --out)")
    ap.add_argument("--workers", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--timeout", type=int, default=1800)
    ap.add_argument("--heap-mb", type=int, default=6000)
    a = ap.parse_args()
    if a.list:
        print("\n".join(MUTANTS)); return 0
    names = a.names or list(MUTANTS)
    unknown = [n for n in names if n not in MUTANTS]
    if unknown:
        print("unknown mutant(s):", unknown, file=sys.stderr); return 2
    out = os.path.abspath(a.out)
    os.makedirs(out, exist_ok=True)
    work = os.path.abspath(a.work) if a.work else out
    os.makedirs(work, exist_ok=True)
    real_hash = sha(os.path.join(HERE, SPEC))
    bad = False
    rows = []

    def parse_ok(d):
        if not a.jar:
            return True, "(SANY skipped: no --jar)"
        return tlc_runner.sany(a.java, a.jar, d)

    # Baseline: an unedited copy must stage, parse, and (with --baseline) pass.
    d = stage(os.path.join(work, "_baseline"))
    if sha(os.path.join(d, SPEC)) != real_hash:
        print("BASELINE FAIL: the staged copy differs from the real spec"); return 1
    ok, txt = parse_ok(d)
    if not ok:
        print("BASELINE FAIL: the unmutated copy does not parse:\n" + txt); return 1
    if a.baseline and not a.dry:
        r = tlc_runner.run_tlc(a.java, a.jar, d, "cfg/smoke-rest.cfg", os.path.join(out, "_baseline.tlc.txt"),
                               a.workers, a.timeout, a.heap_mb)
        print(f"BASELINE {r.status} smoke-rest states={r.distinct} secs={r.seconds:.0f} {r.what}")
        if r.status != "PASS":
            return 1
    else:
        print("BASELINE staged and parsed" + (" (dry)" if a.dry else ""))

    for name in names:
        desc, edits, cfgs, expect = MUTANTS[name]
        d, err = make_mutant(name, work)
        if err is None and sha(os.path.join(d, SPEC)) == real_hash:
            err = "the edit changed nothing"
        if err is None:
            ok, txt = parse_ok(d)
            if not ok:
                err = "the mutant does not parse: " + txt.strip().splitlines()[-1][:160]
        if err:
            rows.append((name, "INVALID", err, "", 0, 0.0)); bad = True; continue
        if a.dry:
            rows.append((name, "DRY-OK", "edit applies once; parses", "", 0, 0.0)); continue
        verdict = ("SURVIVED", "passed on every configuration: " + ", ".join(cfgs), "", 0, 0.0)
        for c in cfgs:
            r = tlc_runner.run_tlc(a.java, a.jar, d, f"cfg/{c}.cfg", os.path.join(out, f"{name}.{c}.tlc.txt"),
                                   a.workers, a.timeout, a.heap_mb)
            if r.status == "VIOLATED":
                note = "" if r.what in expect else f" (expected one of {expect})"
                verdict = ("CAUGHT", f"{r.what}{note}", c, r.distinct, r.seconds); break
            if r.status != "PASS":
                verdict = ("INVALID", f"{c}: {r.status} {r.what}", c, r.distinct, r.seconds); break
        rows.append((name,) + verdict)
        if verdict[0] != "CAUGHT":
            bad = True
        # The real spec must still be byte-identical after every mutant.
        if sha(os.path.join(HERE, SPEC)) != real_hash:
            print("FATAL: the real spec was modified"); return 3

    print(f"\n{'mutant':34} {'result':9} {'violated invariant / reason':60} {'config':16} {'distinct':>10} {'secs':>6}")
    for name, res, what, cfg, dist, secs in rows:
        print(f"{name:34} {res:9} {what[:60]:60} {cfg:16} {dist:>10} {secs:6.0f}")
    counts = {k: sum(1 for r in rows if r[1] == k) for k in ("CAUGHT", "SURVIVED", "INVALID", "DRY-OK")}
    print("\nSUMMARY " + " ".join(f"{k}={v}" for k, v in counts.items()) + f" of {len(rows)} (required mutants: {len(REQUIRED)})")
    missing = [n for n in REQUIRED if n not in MUTANTS]
    if missing:
        print("MISSING REQUIRED MUTANTS:", missing); bad = True
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main())
