#!/usr/bin/env python3
"""Static checks of IcebergCommit.tla that need no model checker (a TLC run would report the same mistakes only after
starting):
  1. every leaf action assigns every state variable exactly once (as x' = ..., x' \\in ..., or inside UNCHANGED);
  2. every MUTATION POINT definition appears exactly once, and every mutant edit of mutate_model.py targets one;
  3. every invariant and canary named in a cfg file exists in the spec.
usage: lint_spec.py [dir]   (default: this directory)"""
import os, re, sys
HERE = os.path.dirname(os.path.abspath(__file__))
d = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else HERE
sys.path.insert(0, HERE)
spec = open(os.path.join(d, "IcebergCommit.tla")).read()
errors = []

m = re.search(r"^VARIABLES\s*(.*?)\n\nledger", spec, re.S | re.M)
body = re.sub(r"\\\*.*", "", m.group(1))
variables = [v.strip() for v in re.split(r"[,\s]+", body) if v.strip() and not v.startswith("(*")]
variables = [v for v in variables if re.fullmatch(r"[A-Za-z_]\w*", v)]
groups = {}
for gm in re.finditer(r"^(\w+)\s*==\s*<<([^>]*)>>", spec, re.M):
    if gm.group(1) in ("ledger", "filev", "netv", "wl", "bud", "vars"):
        groups[gm.group(1)] = [x.strip() for x in gm.group(2).split(",")]
def expand(names):
    out = []
    for n in names:
        out += expand(groups[n]) if n in groups and n != "vars" else [n]
    return out
assert sorted(set(expand(groups["vars"]))) == sorted(variables), (variables, groups)

leaf = ["PlanOp", "BeginUpload", "FinishUpload", "AwaitUpload", "WriteMeta", "Send", "Receive", "Timeout",
        "RecoverDone", "RecoverAbort", "RecoverFail", "RecoverRetry", "Crash", "DeliverOk", "DeliverConflict",
        "Drop", "GCOrphan", "PinReader"]
for a in leaf:
    am = re.search(r"^" + a + r"\(?[^=\n]*\)?\s*==\n(.*?)(?=^\S)", spec, re.S | re.M)
    if not am:
        errors.append(f"action {a} not found"); continue
    text = re.sub(r"\\\*.*", "", am.group(1))
    assigned = re.findall(r"\b(\w+)'\s*(?:=|\\in)", text)
    if "Answer(" in text:          # Answer(q, res) is the operator  resp' = ...
        assigned.append("resp")
    unchanged = []
    for um in re.finditer(r"UNCHANGED\s*<<([^>]*)>>", text):
        unchanged += expand([x.strip() for x in um.group(1).split(",")])
    got = assigned + unchanged
    for v in variables:
        n = got.count(v)
        if n != 1:
            errors.append(f"action {a}: variable {v} is assigned {n} times")
    for v in set(got):
        if v not in variables:
            errors.append(f"action {a}: unknown variable {v}")

points = re.findall(r"^(\w+)(?:\([^)]*\))? ==.*MUTATION POINT", spec, re.M)
if len(points) != len(set(points)) or len(points) < 8:
    errors.append(f"mutation points: {points}")
import mutate_model
for name, (desc, edits, cfgs, expect) in mutate_model.MUTANTS.items():
    for old, new in edits:
        if spec.count(old) != 1:
            errors.append(f"mutant {name}: edit target occurs {spec.count(old)} times")
    for c in cfgs:
        if not os.path.exists(os.path.join(d, "cfg", c + ".cfg")):
            errors.append(f"mutant {name}: no cfg {c}")
    for e in expect:
        if not re.search(r"^" + e + r"\b", spec, re.M):
            errors.append(f"mutant {name}: unknown invariant {e}")
for f in sorted(os.listdir(os.path.join(d, "cfg"))):
    t = open(os.path.join(d, "cfg", f)).read()
    sec = re.findall(r"^(?:INVARIANTS|PROPERTIES)\n((?:    \w+\n)+)", t, re.M)
    for blk in sec:
        for n in blk.split():
            if not re.search(r"^" + n + r"\b", spec, re.M):
                errors.append(f"cfg {f}: {n} is not defined in the spec")
print("lint_spec:", "OK" if not errors else "FAILED")
for e in errors:
    print("  ", e)
sys.exit(1 if errors else 0)
