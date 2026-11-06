"""Summary of spill-bench.sh's rounds.csv: per (query, threads), each arm's median wall time and peak RSS, and the paired
ratio arm / mem per round (median [q1, q3]). The twin `mem2` shows the noise: a difference whose quartile range overlaps
the twin's is NOISY. Exit 1 if a run failed or if the row counts of one query differ between arms or rounds."""
import csv, statistics, sys
from collections import defaultdict

rows = list(csv.DictReader(open(sys.argv[1])))
bad = 0
for r in rows:
    if r["status"] != "ok":
        print(f"FAIL run {r['query']} t={r['threads']} {r['arm']} round {r['round']}: status {r['status']}")
        bad = 1
counts = defaultdict(set)
for r in rows:
    if r["status"] == "ok" and r["rows"]:
        counts[r["query"]].add(r["rows"])
for q, c in counts.items():
    if len(c) != 1:
        print(f"FAIL {q}: row counts differ between arms/rounds: {sorted(c)}")
        bad = 1

def quart(xs):
    xs = sorted(xs)
    if len(xs) < 2:
        return xs[0], xs[0], xs[0]
    q = statistics.quantiles(xs, n=4, method="inclusive")
    return q[0], statistics.median(xs), q[2]

blocks = defaultdict(lambda: defaultdict(dict))
for r in rows:
    if r["status"] == "ok":
        blocks[(r["query"], int(r["threads"]))][r["arm"]][int(r["round"])] = r
print("| query | threads | arm | median s | median peak RSS MB | spilled MB | spill files | most open | arm / mem median [q1, q3] | verdict |")
print("|---|---:|---|---:|---:|---:|---:|---:|---|---|")
for (q, t) in sorted(blocks):
    arms = blocks[(q, t)]
    base = arms.get("mem", {})
    twin = None
    if "mem2" in arms:
        tr = [float(arms["mem2"][k]["secs"]) / float(base[k]["secs"]) for k in arms["mem2"] if k in base and float(base[k]["secs"]) > 0]
        twin = quart(tr) if tr else None
    for a in ["mem", "mem2", "g256", "g64", "duckdb"]:
        if a not in arms:
            continue
        rs = arms[a]
        secs = [float(x["secs"]) for x in rs.values()]
        rss = [int(x["peak_rss_mb"]) for x in rs.values()]
        sp = rs[min(rs)].get("spilled_mb") or "-"
        sf = rs[min(rs)].get("spill_files") or "-"
        mo = rs[min(rs)].get("max_open_files") or "-"
        ratio, verdict = "-", "-"
        if a != "mem" and base:
            pr = [float(rs[k]["secs"]) / float(base[k]["secs"]) for k in rs if k in base and float(base[k]["secs"]) > 0]
            if pr:
                lo, med, hi = quart(pr)
                ratio = f"{med:.2f} [{lo:.2f}, {hi:.2f}]"
                if a == "mem2":
                    verdict = "control"
                elif twin and lo <= twin[2] and hi >= twin[0]:
                    verdict = "NOISY (overlaps the twin)"
                else:
                    verdict = "slower" if med > 1 else ("faster" if med < 1 else "equal")
        print(f"| {q} | {t} | {a} | {statistics.median(secs):.3f} | {statistics.median(rss)} | {sp} | {sf} | {mo} | {ratio} | {verdict} |")
sys.exit(bad)
