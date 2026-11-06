#!/usr/bin/env python3
"""Turn one trace of the Rust writers (src/trace.rs, IC_TRACE=<dir>) into a TLC run of model/TraceSpec.tla.

usage: trace_to_tla.py TRACE.jsonl OUTDIR [--dump]
Writes OUTDIR/Tr.tla (the constants and the abstract event sequence) and OUTDIR/Tr.cfg. Exit 0; exit 3 when the trace
holds something the model has no action for (UNMODELLED: never a verdict on the code), exit 4 when the file is not a trace.

How the raw events become model actions (the mapping is also written up in bench/results/20261104-iceberg-commit.md):
  plan            PlanOp(w)            the read of the table the operation is based on; the model derives the plan
  upload-begin    BeginUpload(w)       before the data / delete file is written
  upload-finish   FinishUpload(file)   the file is durable (an environment action in the model)
  upload-await    AwaitUpload(w)       data or delete file AND its manifests are durable, the writer goes on
  build           WriteMeta(w)         the manifest list of attempt k is written, built from table version `base`
  send            Send(w)              the commit request leaves, expecting table version `exp`
  deliver         DeliverOk/Conflict   the catalog handles a request (store: the conditional create); `cur` is the
                                       version the catalog was at, `new` the version it created
  drop            Drop(q)              the fault injector loses a request before the catalog sees it
  receive         Receive(w)           the writer reads the answer (ok / conflict)
  timeout         Timeout(w)           the writer gets an unknown outcome (a lost response, or a lost request)
  recover-*       RecoverDone/Abort/Retry/Fail   the decision after re-reading the table, at the position of that read
  crash           Crash(w)             the writer dies at a crash point
  outcome         (no action)          what the writer reports must be the model's final state of that writer
Model writer numbers are given to the operations (token w<id>o<ordinal>) in the order of their `plan` events. The seed
commit of every scenario (writer 0) is the model's initial state: its rows are InitRows, its table version is model
version 0, and all versions are shifted by it.
Three things are done to the sequence before it is written, each of them a decision to review:
  * an intent without an outcome (the process was killed inside the catalog call) is resolved from the next version
    anyone observes: applied (create succeeded), or not (the create never happened);
  * the SECOND delivery of a duplicated request, when it is a conflict (the first one created the version), is moved to
    just after the writer reads its answer, because the real client never reads the duplicate's answer while the model's
    single response slot would let that conflict overwrite the first, "ok" answer; its `cur` becomes the version at its
    new place (a conflict stays a conflict: the version only grows);
  * writers' soft and hard crashes are the same event.
"""
import json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))

class Unmodelled(Exception):
    """The trace holds an event the model has no action for."""

class BadTrace(Exception):
    """The file is not a well-formed trace."""

KNOWN = {"meta", "final", "planned", "plan", "upload-begin", "upload-finish", "upload-await", "build", "send", "deliver-intent", "deliver", "drop",
         "receive", "timeout", "rejected", "recover-done", "recover-abort", "recover-retry", "recover-fail", "crash", "outcome"}
PC_OF_OUTCOME = {"acked": "done", "recovered": "done", "aborted": "aborted", "gave_up": "failed", "crashed": "crashed"}

def read_events(path):
    """(events in file order, number of torn lines). A torn last line (a writer killed mid-write) is ignored."""
    evs, torn = [], 0
    for n, line in enumerate(open(path, errors="replace")):
        line = line.strip()
        if not line:
            continue
        try:
            e = json.loads(line)
        except ValueError:
            torn += 1
            continue
        if not isinstance(e, dict) or "a" not in e or "seq" not in e:
            raise BadTrace(f"{path}:{n + 1}: not a trace event")
        if e["a"] not in KNOWN:
            raise Unmodelled(f"unknown event kind {e['a']!r}")
        e["_n"] = n
        evs.append(e)
    return evs, torn

def ordered(evs):
    return sorted(evs, key=lambda e: (e["seq"], e.get("sub", 1), e["_n"]))

def resolve_intents(evs):
    """Pair every deliver-intent with its deliver line (same seq). An intent alone: the process died inside the catalog call."""
    done = {e["seq"] for e in evs if e["a"] == "deliver"}
    out = []
    for k, e in enumerate(evs):
        if e["a"] != "deliver-intent":
            out.append(e)
            continue
        if e["seq"] in done:
            continue
        # killed inside the call: look at the next observation of the catalog's version
        seen = None
        for f in evs[k + 1:]:
            if f["a"] in ("plan", "recover-done", "recover-abort", "recover-retry", "recover-fail", "final") and "ver" in f:
                seen = f["ver"]
                break
            if f["a"] == "deliver":
                seen = f["cur"]
                break
        r = dict(e, a="deliver", resolved=True)
        if e["cur"] > e["exp"]:
            r["res"] = "conflict"                      # the create could not succeed: the version existed
        elif seen is not None and seen >= e["exp"] + 1:
            r["res"] = "ok"                            # the create happened (nobody else could have made that version)
            r["new"] = e["exp"] + 1
        else:
            continue                                   # the create never happened: the request stays in the network
        out.append(r)
    return out

def commute_duplicates(evs):
    """Move the second delivery of a duplicated request to just after the writer reads its answer (see the module doc)."""
    evs = list(evs)
    k = 0
    while k < len(evs):
        e = evs[k]
        if e["a"] == "deliver" and e.get("dup") == 2 and e.get("res") == "conflict":   # MUTATION POINT: duplicate delivery order
            for j in range(k + 1, len(evs)):
                f = evs[j]
                if f["a"] == "receive" and f["op"] == e["op"] and f["att"] == e["att"]:
                    e["moved"] = True                                                 # `cur` is no longer what the catalog was at
                    evs.pop(k)                                                        # the receive is now at j - 1
                    evs.insert(j, e)
                    k -= 1
                    break
        k += 1
    return evs

def tla_str(s):
    return json.dumps(str(s))

def translate(path):
    """-> dict(trace=[abstract events], consts=..., meta=..., notes=[...]). Raises Unmodelled / BadTrace."""
    raw, torn = read_events(path)
    evs = commute_duplicates(resolve_intents(ordered(raw)))
    per_seq = {}
    for e in raw:
        if e["a"] != "deliver-intent":
            per_seq.setdefault(e["seq"], []).append(e)
    for seq, group in sorted(per_seq.items()):
        if len([e for e in group if e["a"] != "deliver"]) + (1 if any(e["a"] == "deliver" for e in group) else 0) > 1:
            raise BadTrace(f"sequence number {seq} is used by more than one event ({', '.join(e['a'] for e in group)}): the trace's counter was damaged")
    meta = next((e for e in evs if e["a"] == "meta"), {})
    final = [e for e in evs if e["a"] == "final"]
    ops = [e for e in evs if e["a"] not in ("meta", "final")]
    # ---- the seed commit is the initial state
    seed = [e for e in ops if e["op"].startswith("w0o")]
    if not seed:
        raise BadTrace("no seed commit (writer 0) in the trace")
    sp = next((e for e in seed if e["a"] == "plan"), None)
    sd = next((e for e in seed if e["a"] == "deliver" and e.get("res") == "ok"), None)
    so = next((e for e in seed if e["a"] == "outcome"), None)
    if not (sp and sd and so and so["res"] == "acked"):
        raise BadTrace("the seed commit (writer 0) did not complete")
    v0 = sd["new"]                                                                    # MUTATION POINT: version offset
    init_rows = [(r[0], r[1]) for r in sp["rows"]]
    rest = [e for e in ops if not e["op"].startswith("w0o")]
    last_seed = max(e["seq"] for e in seed)
    if any(e["seq"] < last_seed for e in rest):
        raise BadTrace("an event of an operation precedes the end of the seed commit")
    # ---- model writers in the order of their plan events
    writers, plans = {}, {}
    for e in rest:
        if e["a"] == "plan":
            writers.setdefault(e["op"], len(writers) + 1)
            plans[e["op"]] = e
    for e in rest:
        if e["op"] not in writers:
            raise BadTrace(f"event {e['a']} of {e['op']} without a plan event")
    isolations = {p["isolation"] for p in plans.values()}
    if len(isolations) > 1:
        raise Unmodelled(f"writers with different isolation levels {sorted(isolations)}: the model has one")
    tr = []
    now = 0                                                                           # the model's current version, from the deliveries so far
    for e in rest:
        w, a = writers[e["op"]], e["a"]
        if a == "plan":
            tr.append(dict(a=a, w=w, ver=e["ver"] - v0))
        elif a in ("upload-begin", "upload-finish", "upload-await", "crash"):
            tr.append(dict(a=a, w=w))
        elif a == "build":
            tr.append(dict(a=a, w=w, att=e["att"], base=e["base"] - v0))
        elif a == "send":
            tr.append(dict(a=a, w=w, att=e["att"], exp=e["exp"] - v0))
        elif a == "deliver":
            if e["res"] == "error":
                raise Unmodelled(f"the catalog returned an error other than a conflict ({e.get('err', '?')[:80]})")
            if e["res"] == "ok" and "new" not in e:
                raise BadTrace("an accepted commit without the version it created")
            if e.get("moved") and e["res"] != "conflict":
                raise BadTrace("a moved duplicate delivery that is not a conflict")
            cur = now if e.get("moved") else e["cur"] - v0          # a moved delivery is a conflict whatever the version is by then
            if e["res"] == "ok":
                now = e["new"] - v0
            tr.append(dict(a=a, w=w, att=e["att"], exp=e["exp"] - v0, cur=cur, res=e["res"],
                           new=(e["new"] - v0 if e["res"] == "ok" else 0), keep=bool(e.get("keep"))))
        elif a == "drop":
            tr.append(dict(a=a, w=w, att=e["att"]))
        elif a == "receive":
            tr.append(dict(a=a, w=w, att=e["att"], res=e["res"]))
        elif a == "timeout":
            tr.append(dict(a=a, w=w, att=e["att"]))
        elif a.startswith("recover-"):
            tr.append(dict(a=a, w=w, att=e["att"], **{"from": e["from"]}, ver=e["ver"] - v0))
        elif a == "planned":
            tr.append(dict(a=a, w=w, targets=e["targets"], inputs=e["inputs"], rows=e["rows"]))
        elif a == "outcome":
            if e["res"] not in PC_OF_OUTCOME:
                raise Unmodelled(f"outcome {e['res']!r} of {e['op']} (the model has no such end state)")
            tr.append(dict(a=a, w=w, pc=PC_OF_OUTCOME[e["res"]]))
        elif a == "rejected":
            raise Unmodelled(f"{e['op']}: the catalog refused the commit with {e.get('code')} (the model has no such answer)")
        elif a == "deliver-intent":
            raise BadTrace("unresolved intent")
    for f in final:
        if "error" in f:
            raise Unmodelled(f"the final state of the table could not be read: {f['error'][:100]}")
        if f["ver"] < v0:
            raise BadTrace("final version below the seed")
        tr.append(dict(a="final", w=0, ver=f["ver"] - v0, rows=sorted(f["rows"])))
    # ---- constants
    kinds, app, dele, keyof = {}, {}, {}, dict(init_rows)
    for op, w in writers.items():
        p = plans[op]
        kinds[w] = p["kind"]
        app[w] = [r[0] for r in p["rows"]]
        for r in p["rows"]:
            keyof[r[0]] = r[1]
        dele[w] = p["keys"][0] if p["kind"] == "delete" else "-"
    n_deliver_ok = sum(1 for t in tr if t["a"] == "deliver" and t["res"] == "ok")
    consts = dict(
        writers=sorted(writers.values()), kinds=kinds, app=app, dele=dele, keyof=keyof,
        init_rows=[r[0] for r in init_rows], isolation=isolations.pop() if isolations else "serializable",
        max_attempts=max([p["max_attempts"] for p in plans.values()] or [1]),
        max_ver=n_deliver_ok + 5,
        crash=sum(1 for t in tr if t["a"] == "crash"),
        dup=sum(1 for t in tr if t["a"] == "deliver" and t["keep"]),
        loss=sum(1 for t in tr if t["a"] == "drop"))
    return dict(trace=tr, consts=consts, meta=meta, writers={w: op for op, w in writers.items()}, torn=torn, version_offset=v0)

def fn(d, val):
    """A TLA+ function literal (k1 :> v1) @@ (k2 :> v2) ..."""
    items = [f"({k} :> {val(v)})" for k, v in sorted(d.items())]
    return " @@ ".join(items) if items else "[x \\in {} |-> 0]"

def tla_val(v):
    if isinstance(v, bool):
        return "TRUE" if v else "FALSE"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, list):
        return "{" + ", ".join(tla_val(x) for x in v) + "}"
    return tla_str(v)

def render(t, outdir):
    os.makedirs(outdir, exist_ok=True)
    c = t["consts"]
    ev = []
    for e in t["trace"]:
        ev.append("[" + ", ".join(f"{k} |-> {tla_val(v)}" for k, v in e.items()) + "]")
    lines = ["---- MODULE Tr ----", "EXTENDS TraceSpec, TLC", "",
             "TrOp == " + fn(c["kinds"], tla_str),
             "TrApp == " + fn(c["app"], lambda rows: "{" + ", ".join(str(r) for r in rows) + "}"),
             "TrDel == " + fn(c["dele"], tla_str),
             "TrKey == " + fn(c["keyof"], tla_str),
             "TrTrace == <<"]
    lines += ["    " + ",\n    ".join(ev) if ev else "", ">>", "====", ""]
    open(os.path.join(outdir, "Tr.tla"), "w").write("\n".join(lines))
    rows = sorted(set(c["keyof"]))
    invs = ["TypeOK", "I1_FilesExist", "I2_NoLostUpdate"] + (["I4_NoPhantom"] if c["isolation"] == "serializable" else []) \
        + ["I5_AtMostOnce", "I6_SeqIncreases", "I7_NoSnapshotLost", "NotDone"]
    cfg = ["SPECIFICATION TraceSpec", "CONSTANTS",
           "    Writers = {" + ", ".join(str(w) for w in c["writers"]) + "}",
           "    OpOf <- TrOp", "    AppendRows <- TrApp", "    DelKey <- TrDel",
           "    Rows = {" + ", ".join(str(r) for r in rows) + "}",
           "    KeyOf <- TrKey",
           "    InitRows = {" + ", ".join(str(r) for r in c["init_rows"]) + "}",
           '    Style = "STORE"', f'    Isolation = "{c["isolation"]}"',
           f"    MaxAttempts = {c['max_attempts']}", f"    MaxVer = {c['max_ver']}", "    MaxPins = 0",
           f"    CrashBudget = {c['crash']}", f"    DupBudget = {c['dup']}", f"    LossBudget = {c['loss']}",
           "    Trace <- TrTrace", "INVARIANTS"] + ["    " + i for i in invs] + [""]
    open(os.path.join(outdir, "Tr.cfg"), "w").write("\n".join(cfg))

def main(argv):
    if len(argv) < 3:
        print(__doc__.split("\n\n")[0], file=sys.stderr)
        return 2
    try:
        t = translate(argv[1])
    except Unmodelled as e:
        print(f"UNMODELLED: {e}")
        return 3
    except BadTrace as e:
        print(f"BAD TRACE: {e}")
        return 4
    render(t, argv[2])
    if "--dump" in argv:
        print(json.dumps(t, indent=1, default=str))
    else:
        print(f"{len(t['trace'])} events, {len(t['writers'])} operations, version offset {t['version_offset']}")
    return 0

if __name__ == "__main__":
    sys.exit(main(sys.argv))
