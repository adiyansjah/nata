"""sqlfuzz: differential SQL fuzzer. Commands: run, prove, replay, gen. See ../README.md."""
import argparse, json, os, random, shlex, sys, time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass

from . import gen
from .compare import compare
from .engine import Command, DuckDB
from .faults import FAULTS
from .render import Join, Select, SetOp
from .shrink import shrink

NO = frozenset()


def kind_of(q):
    if isinstance(q, SetOp): return "setop"
    f = q.frm
    if isinstance(f, Join): return "join-" + f.kind.lower()
    if q.group or any(type(i.e).__name__ == "Agg" for i in q.items): return "aggregate"
    if any(type(i.e).__name__ == "Win" for i in q.items): return "window"
    if type(q.where).__name__ == "Exists": return "semi-anti"
    return "project"


def build(seed, batch, per_db, max_rows):
    rng = random.Random(seed * 1_000_003 + batch)
    tables = gen.make_tables(rng, max_rows)
    return tables, [gen.gen_query(rng) for _ in range(per_db)]


def target_for(args, faults):
    if args.target_cmd: return Command(shlex.split(args.target_cmd))
    extra = []
    for f in faults: extra += FAULTS[f][1] or []
    return DuckDB(args.duckdb, session_extra=extra)


@dataclass
class Diff:
    seed: int
    batch: int
    qid: int
    kind: str
    reason: str
    sql: str


def run_batch(args, seed, batch, faults):
    """Returns (stats, diffs). Oracle = faithful DuckDB; target = DuckDB with `faults`, or the command adapter."""
    tables, queries = build(seed, batch, args.per_db, args.max_rows)
    setup = gen.setup_sql(tables)
    F = frozenset(faults)
    oracle, target = DuckDB(args.duckdb), target_for(args, faults)
    with ThreadPoolExecutor(2) as ex:
        fo = ex.submit(oracle.run, setup, [(i, q.sql(NO)) for i, q in enumerate(queries)])
        ft = ex.submit(target.run, setup, [(i, q.sql(F)) for i, q in enumerate(queries)])
        ro, rt = fo.result(), ft.result()
    st = {"queries": len(queries), "oracle_errors": 0, "skipped": 0, "diffs": 0, "by_kind": {}}
    diffs = []
    for i, q in enumerate(queries):
        k = kind_of(q); b = st["by_kind"].setdefault(k, [0, 0]); b[0] += 1
        if not ro[i].ok: st["oracle_errors"] += 1; b[1] += 1
        why = compare(ro[i], rt[i], getattr(q, "ordered", False), args.strict_types)
        if why == "SKIP": st["skipped"] += 1
        elif why: st["diffs"] += 1; diffs.append(Diff(seed, batch, i, k, why, q.sql(F)))
    return st, diffs, (tables, queries)


def merge(a, b):
    for k in ("queries", "oracle_errors", "skipped", "diffs"): a[k] += b[k]
    for k, (n, e) in b["by_kind"].items():
        x = a["by_kind"].setdefault(k, [0, 0]); x[0] += n; x[1] += e


def new_stats(): return {"queries": 0, "oracle_errors": 0, "skipped": 0, "diffs": 0, "by_kind": {}}


def reason_class(why):
    """The kind of difference: keeps the shrinker on the bug it started with (a simpler query that fails for another reason is not it)."""
    return " ".join(why.split(" ")[:3])


def make_checker(args, faults, cls=None):
    F = frozenset(faults); oracle, target = DuckDB(args.duckdb), target_for(args, faults)

    def still_fails(query, tables):
        setup = gen.setup_sql(tables)
        ro = oracle.run(setup, [(0, query.sql(NO))])[0]
        if not ro.ok: return False       # a shrunk query that no longer runs on the oracle is not the same bug
        rt = target.run(setup, [(0, query.sql(F))])[0]
        why = compare(ro, rt, getattr(query, "ordered", False), args.strict_types)
        return bool(why) and why != "SKIP" and (cls is None or reason_class(why) == cls)
    return still_fails


def write_repro(args, name, d, query, tables, faults, steps):
    os.makedirs(args.out, exist_ok=True)
    base = os.path.join(args.out, name)
    F = frozenset(faults)
    with open(base + ".sql", "w") as f:
        f.write(f"-- sqlfuzz reproducer (seed {d.seed}, batch {d.batch}, query {d.qid}, kind {d.kind}); faults on the target: {sorted(faults) or 'none'}\n")
        f.write(f"-- difference: {d.reason}\n-- shrunk in {steps} checks\n")
        for s in gen.setup_sql(tables): f.write(s + ";\n")
        f.write(f"-- oracle query:\n{query.sql(NO)};\n")
        if F: f.write(f"-- target query:\n-- {query.sql(F)};\n")
    return base + ".sql"


def cmd_run(args):
    faults = [x for x in (args.fault or "").split(",") if x]
    stats, found, t0 = new_stats(), [], time.time()
    batches = max(1, args.cases // args.per_db)
    with ThreadPoolExecutor(args.jobs) as ex:
        futs = [ex.submit(run_batch, args, args.seed, b, faults) for b in range(batches)]
        for b, fu in enumerate(futs):
            st, diffs, case = fu.result()
            merge(stats, st); found += [(d, case) for d in diffs]
    print(f"queries {stats['queries']}  oracle-errors {stats['oracle_errors']} ({100 * stats['oracle_errors'] // max(1, stats['queries'])}%)  "
          f"target-not-supported {stats['skipped']}  differences {stats['diffs']}  [{time.time() - t0:.0f}s]")
    for k, (n, e) in sorted(stats["by_kind"].items()): print(f"  {k:12} {n:6} queries, {e:5} oracle errors")
    for d, (tables, queries) in found[:args.shrink]:
        sh = shrink(queries[d.qid], tables, make_checker(args, faults, reason_class(d.reason)))
        p = write_repro(args, f"diff-seed{d.seed}-b{d.batch}-q{d.qid}", d, sh[0], sh[1], faults, sh[2])
        print(f"DIFF {d.kind}: {d.reason[:160]}\n     shrunk to {len(sh[0].sql(NO))} chars, rows {[len(v) for v in sh[1].values()]} -> {p}")
    if found and not args.shrink:
        for d, _ in found[:5]: print(f"DIFF {d.kind}: {d.reason[:160]}\n     {d.sql[:200]}")
    return 1 if stats["diffs"] else 0


def rejected(d):
    return d.reason.startswith("target raised") and ("Parser Error" in d.reason or "Binder Error" in d.reason)


def cmd_prove(args):
    """Control (no fault) must be clean; every fault must be caught within --cases queries."""
    bad = 0; lines = []
    st, diffs, _ = run_batch(args, args.seed, 0, [])
    ctl_queries = 0; ctl_diffs = 0
    with ThreadPoolExecutor(args.jobs) as ex:
        for st, diffs, _ in ex.map(lambda b: run_batch(args, args.seed, b, []), range(max(1, args.control // args.per_db))):
            ctl_queries += st["queries"]; ctl_diffs += st["diffs"]
    lines.append(f"control (faithful rendering vs oracle): {ctl_queries} queries, {ctl_diffs} differences -> {'ok' if ctl_diffs == 0 else 'FAIL'}")
    bad += ctl_diffs != 0
    names = [f for f in (args.faults.split(",") if args.faults != "all" else FAULTS) if f]
    maxb = max(1, args.cases // args.per_db)

    def one(name):
        seen = 0
        for b in range(maxb):
            st, diffs, case = run_batch(args, args.seed, b, [name])
            real = [d for d in diffs if not rejected(d)]
            if diffs and not real:
                return name, -1, diffs[0].reason[:90], 0, 0, ""      # the engine rejects the faulty rendering: the fault itself is broken
            diffs = real
            if diffs:
                d = diffs[0]
                sh = shrink(case[1][d.qid], case[0], make_checker(args, [name], reason_class(d.reason)), max_checks=args.shrink_checks)
                p = write_repro(args, f"fault-{name}", d, sh[0], sh[1], [name], sh[2])
                return name, seen + d.qid + 1, d.kind, len(sh[0].sql(NO)), sum(len(v) for v in sh[1].values()), p
            seen += st["queries"]
        return name, None, "", 0, 0, ""
    with ThreadPoolExecutor(args.jobs) as ex:
        res = list(ex.map(one, names))
    for name, n, kind, size, rows, p in res:
        if n == -1: lines.append(f"INVALID  {name:34} the faulty rendering is rejected by the engine: {kind}"); bad += 1
        elif n is None: lines.append(f"SURVIVED {name:34} not caught in {args.cases} queries (the grammar or data cannot reach it)"); bad += 1
        else: lines.append(f"caught   {name:34} after {n:5} queries ({kind}); shrunk to {size} chars, {rows} rows -> {os.path.basename(p)}")
    print("\n".join(lines))
    print(f"{sum(1 for r in res if r[1] not in (None, -1))} of {len(names)} faults caught; control {'clean' if ctl_diffs == 0 else 'NOT clean'}")
    return 1 if bad else 0


def cmd_gen(args):
    rng = random.Random(args.seed)
    for _ in range(args.count): print(gen.gen_query(rng).sql(NO) + ";")
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(prog="sqlfuzz")
    ap.add_argument("cmd", choices=["run", "prove", "gen", "run-subset", "prove-subset", "replay-subset"])
    ap.add_argument("--oracle-optimizer", action="store_true", help="run-subset: leave DuckDB's optimizer on in the oracle")
    ap.add_argument("--batch", type=int, default=0, help="replay-subset: the batch of the seed")
    ap.add_argument("--query", type=int, default=-1, help="replay-subset: one query of the batch (default: all)")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--cases", type=int, default=2000)
    ap.add_argument("--per-db", type=int, default=40)
    ap.add_argument("--max-rows", type=int, default=25)
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
    ap.add_argument("--fault", default="", help="comma list of faults for the target (run)")
    ap.add_argument("--faults", default="all", help="prove: comma list or all")
    ap.add_argument("--control", type=int, default=2000, help="prove: queries of the clean control run")
    ap.add_argument("--target-cmd", default="", help="adapter command for a real target (see README)")
    ap.add_argument("--duckdb", default="duckdb")
    ap.add_argument("--strict-types", action="store_true")
    ap.add_argument("--shrink", type=int, default=400, help="run: how many differences to shrink and write")
    ap.add_argument("--shrink-checks", type=int, default=250)
    ap.add_argument("--out", default="sqlfuzz-out")
    ap.add_argument("--count", type=int, default=10)
    args = ap.parse_args(argv)
    if args.cmd in ("run-subset", "prove-subset", "replay-subset"):
        from . import subset_cli
        return {"run-subset": subset_cli.cmd_run, "prove-subset": subset_cli.cmd_prove, "replay-subset": subset_cli.cmd_replay}[args.cmd](args)
    return {"run": cmd_run, "prove": cmd_prove, "gen": cmd_gen}[args.cmd](args)
