"""Assembles bench/results/20261106-sql-parser-baseline.md from the narrative below and the tables of `report.py` (target/corpus/tables.md, timing.jsonl, ...)."""
import json, os, re, statistics, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lib import *

T = open(os.path.join(OUT, "tables.md")).read()
SEC = {m.group(1): m.group(2) for m in re.finditer(r"<!-- section (\w+) -->\n(.*?)(?=\n<!-- section |\Z)", T, re.S)}

def part(sec, start, end=None):
    s = SEC[sec]
    i = s.index(start)
    j = s.index(end, i + 1) if end else len(s)
    return s[i:j].strip()

# timing
tm = [json.loads(l) for l in open(os.path.join(OUT, "timing.jsonl"))]
clock, qinfo, passes = tm[0]["clock_ns"], tm[1], tm[2:]
rows = []
for arm in ("hand", "a-generic", "a-postgres", "a-generic-parse-only"):
    med = [p["arms"][arm]["median_ns"] for p in passes]
    p99 = [p["arms"][arm]["p99_ns"] for p in passes]
    p999 = [p["arms"][arm]["p999_ns"] for p in passes]
    mx = [p["arms"][arm]["max_ns"] for p in passes]
    f = lambda v: f"{statistics.median(v) / 1000:.2f} ({min(v) / 1000:.2f} to {max(v) / 1000:.2f})"
    rows.append(f"| {arm} | {f(med)} | {f(p99)} | {f(p999)} | {max(mx) / 1000:.1f} |")
TIMING = "| arm | median per query, us (min to max over 5 passes) | p99, us | p99.9, us | worst query, us |\n|---|---|---|---|---|\n" + "\n".join(rows)

tpl = open(os.path.join(HERE, "results_template.md")).read()
out = tpl
out = out.replace("{{TIMING}}", TIMING).replace("{{CLOCK}}", f"{clock:.0f}").replace("{{NQ}}", str(qinfo["queries"])).replace("{{MEANB}}", f"{qinfo['mean_bytes']:.0f}")
for k, a, b in [("CORPUS", "corpus", None)]:
    pass
out = out.replace("{{CORPUS}}", SEC["corpus"].strip())
acc = SEC["accept"]
out = out.replace("{{ACCEPT_A}}", part("accept", "### (a) test set", "### (b) TPC-H").replace("### (a)", "#### (a)"))
out = out.replace("{{ACCEPT_B}}", part("accept", "### (b) TPC-H", "### (c) generated").replace("### (b)", "#### (b)"))
out = out.replace("{{ACCEPT_C}}", part("accept", "### (c) generated", "### agreement").replace("### (c)", "#### (c)"))
out = out.replace("{{AGREE}}", part("accept", "### agreement").replace("### agreement", "#### agreement"))
out = out.replace("{{ERRORS}}", SEC["errors"].strip().replace("### (iii)", "### (iii)"))
out = out.replace("{{LITERALS}}", SEC["literals"].strip())
out = out.replace("{{MUTANTS}}", SEC["mutants"].strip())
out = out.replace("{{ROBUST}}", SEC["robust"].strip())
for f, key in (("unsafe-default.txt", "{{UNSAFE_DEFAULT}}"), ("unsafe-nostacker.txt", "{{UNSAFE_NOSTACKER}}")):
    out = out.replace(key, open(os.path.join(OUT, f)).read().strip())
dst = os.path.join(SPIKE, "..", "..", "bench", "results", "20261106-sql-parser-baseline.md")
open(dst, "w").write(out)
print("wrote", os.path.normpath(dst), len(out))
