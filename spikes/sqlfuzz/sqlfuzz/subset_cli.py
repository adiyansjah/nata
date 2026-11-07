"""Commands of the slice 4 subset fuzzer: `run-subset` (oracle DuckDB, target a command adapter such as adapters/nata_adapter.py), `replay-subset` (one
query of a seed), `prove-subset` (the known-bad engines of faults.py that the subset can express must be caught, the control clean)."""
import json, os, random, re, shlex, sys, time
from collections import Counter
from concurrent.futures import ThreadPoolExecutor

from . import subset
from .cli import NO, reason_class, target_for
from .compare import compare
from .engine import Command, DuckDB
from .faults import FAULTS
from .shrink import shrink

SUBSET_FAULTS = ["nan-is-ieee", "between-exclusive", "limit-off-by-one", "int-overflow-yields-null", "decimal-via-double", "string-compare-case-insensitive", "count-col-counts-nulls", "avg-divides-by-rows", "sum-null-is-zero"]


# DuckDB's optimizer folds constants, reorders conjuncts, rewrites `x * 2 > 0` to `x > 0` and pushes a LIMIT below the projection, which removes (or, with
# the optimizer off, adds) evaluations that raise an error (an overflow, a bad cast). The oracle is run both ways and either answer is accepted.
ORACLE_SESSION = ["PRAGMA disable_optimizer"]


def build(seed, batch, per_db, max_rows):
    """Everything about batch `batch` of `seed` depends on (seed, batch) alone, so a failure replays from the two numbers."""
    rng = random.Random(seed * 1_000_003 + batch)
    tables = subset.make_tables(rng, max_rows)
    generator = subset.aggregate_query if batch % 2 else subset.query
    return tables, [generator(rng) for _ in range(per_db)]


# differences that the dialect page lists (docs/dialect.md section 9, docs/decimal-rules.md section 9): counted in their own classes, never as bugs
LISTED = [("CASE results (", "listed: CASE of decimals beyond 38 digits is a bind error (decimal-rules 9)"),
          ("digits, above the maximum", "listed: decimal result beyond 38 digits is a bind error (decimal-rules 3)"),
          ("scale would be", "listed: decimal result scale beyond 38 is a bind error (decimal-rules 3)")]


def minimum_negation_class(why, sql):
    """Only excuse a reached unary minus of a minimum signed literal, at its reported position.

    The generated spelling is `(- -MIN)`. Its inner literal is INTEGER/BIGINT by value
    (8b.10), so the outer minus must overflow even though DuckDB folds it wider. Merely
    containing that spelling is insufficient: another operation may have failed instead.
    """
    if not why.startswith("target raised an error") or "integer overflow in a unary minus:" not in why:
        return None
    pos = re.search(r"\(line (\d+), column (\d+)\)\s*$", why)
    if not pos: return None
    # The adapter replaces FROM tN with a quoted temporary Parquet path. Errors in
    # WHERE therefore have shifted columns; reconstruct the exact submitted SQL.
    source = re.search(r"error: (/[^\n]+?/(t[12])\.parquet): row group \d+: execution error:", why)
    if source:
        sql = re.sub(r"\bFROM " + source[2] + r"\b", lambda _: "FROM '" + source[1] + "'", sql)
    line, col = map(int, pos.groups())
    lines = sql.splitlines(keepends=True)
    if not 1 <= line <= len(lines) or not 1 <= col <= len(lines[line - 1]): return None
    at = sum(map(len, lines[:line - 1])) + col - 1
    literal = re.match(r"\(\s*-\s+-\s*(2147483648|9223372036854775808)\s*\)", sql[at:])
    if not literal: return None
    typ = "INTEGER" if literal[1] == "2147483648" else "BIGINT"
    if f"the result does not fit {typ}." not in why: return None
    return "listed: negating a minimum signed literal overflows its value-based INTEGER/BIGINT type; DuckDB folds a wider literal (dialect 8b.10, 9)"


def classify(why, target_error="", rows=None, sql=""):
    """The class of a difference, for the report."""
    minimum = minimum_negation_class(why, sql)
    if minimum: return minimum
    for k, name in LISTED:
        if k in why: return name
    if why.startswith("target raised an error"):
        msg = why.split(": ", 1)[-1]
        for k in ("internal error", "target failed", "bind error", "syntax error", "execution error", "cannot", "decimal"):
            if k in msg: return "nata-error: " + k
        return "nata-error: other"
    if why.startswith("oracle raised an error"): return "oracle-error-nata-answered"
    if why.startswith("row count"): return "row-count"
    if why.startswith("column count"): return "column-count"
    return "value"


def run_batch(args, seed, batch, faults=()):
    tables, queries = build(seed, batch, args.per_db, args.max_rows)
    setup = subset.setup_sql(tables)
    F = frozenset(faults)
    oracle = DuckDB(args.duckdb)
    oracle_raw = DuckDB(args.duckdb, session_extra=ORACLE_SESSION[:])
    if args.target_cmd: target = Command(shlex.split(args.target_cmd))
    else:
        extra = []
        for f in faults: extra += FAULTS[f][1] or []
        target = DuckDB(args.duckdb, session_extra=extra)
    with ThreadPoolExecutor(3) as ex:
        fo = ex.submit(oracle.run, setup, [(i, q.sql(NO)) for i, q in enumerate(queries)])
        fr = ex.submit(oracle_raw.run, setup, [(i, q.sql(NO)) for i, q in enumerate(queries)])
        ft = ex.submit(target.run, setup, [(i, q.sql(F)) for i, q in enumerate(queries)])
        ro, rr, rt = fo.result(), fr.result(), ft.result()
    st = {"queries": len(queries), "oracle_errors": 0, "skipped": 0, "diffs": 0, "classes": Counter(), "skip_reasons": Counter(), "rows": 0, "optimizer_dependent": 0, "listed": 0}
    diffs = []
    for i, q in enumerate(queries):
        if not ro[i].ok: st["oracle_errors"] += 1
        st["rows"] += len(ro[i].rows)
        # DuckDB itself answers some queries differently with and without its optimizer (it folds constants, reorders conjuncts and pushes a LIMIT below the
        # projection, which removes or adds evaluations that raise errors): both of its answers are accepted, a target that matches neither is a difference
        if args.target_cmd and rt[i].ok:
            for oracle_result in (ro[i], rr[i]):
                if oracle_result.ok and exact_aggregate_oracle(q, tables, oracle_result):
                    st["classes"]["listed: exact AVG versus DuckDB DOUBLE (dialect 10)"] += 1
        why_on, why_raw = compare(ro[i], rt[i], q.ordered), compare(rr[i], rt[i], q.ordered)
        if compare(ro[i], rr[i], q.ordered): st["optimizer_dependent"] += 1
        why = None if (why_on is None or why_raw is None) else (why_on if why_on == "SKIP" else why_on)
        if why == "SKIP" and why_raw not in (None, "SKIP"): why = why_raw
        if why == "SKIP":
            st["skipped"] += 1; st["skip_reasons"][" ".join(rt[i].error.split()[:8])[:80]] += 1
        elif why:
            if why.startswith("oracle raised an error") and rt[i].ok and not rt[i].rows: why += " [Nata answered with 0 rows]"
            cls = classify(why, rows=len(rt[i].rows) if rt[i].ok else None, sql=q.sql(F))
            st["classes"][cls] += 1
            if cls.startswith("listed"): st["listed"] += 1; continue
            st["diffs"] += 1
            diffs.append((seed, batch, i, why, q.sql(F)))
    return st, diffs, (tables, queries)


def merge(a, b):
    for k in ("queries", "oracle_errors", "skipped", "diffs", "rows", "optimizer_dependent", "listed"): a[k] += b[k]
    a["classes"].update(b["classes"]); a["skip_reasons"].update(b["skip_reasons"])


def checker(args, faults, cls):
    F = frozenset(faults)
    oracle = DuckDB(args.duckdb)
    oracle_raw = DuckDB(args.duckdb, session_extra=ORACLE_SESSION[:])
    if args.target_cmd: target = Command(shlex.split(args.target_cmd))
    else: target = DuckDB(args.duckdb, session_extra=[e for f in faults for e in (FAULTS[f][1] or [])])

    def still_fails(query, tables):
        setup = subset.setup_sql(tables)
        ro = oracle.run(setup, [(0, query.sql(NO))])[0]
        rr = oracle_raw.run(setup, [(0, query.sql(NO))])[0]
        rt = target.run(setup, [(0, query.sql(F))])[0]
        if args.target_cmd and rt.ok:
            for answer in (ro, rr):
                if answer.ok:
                    certified = exact_aggregate_oracle(query, tables, answer)
                    from .render import Agg
                    if any(isinstance(it.e, Agg) and it.e.fn == "avg" for it in query.items) and not certified:
                        return False
        why, why_raw = compare(ro, rt, query.ordered), compare(rr, rt, query.ordered)
        if why_raw is None: return False
        return bool(why) and why != "SKIP" and reason_class(why) == cls
    return still_fails


COLUMNS = re.compile(r"\b(?:id|a|b|ti|sm|c|r|d|w|s|f|dt)\b")


def constant_select_list(sql):
    """True when the select list of the (shrunk) query names no column: it is made of literals."""
    m = re.match(r"SELECT (.*?) FROM ", sql)
    return bool(m) and not COLUMNS.search(re.sub(r"'(?:[^']|'')*'", "''", m.group(1)))


def null_between_cast_difference(why, query, tables):
    """Prove the narrow §9b difference from the shrunk AST AND its rows, not the word BETWEEN.

    Only an unfiltered, unlimited, single CASE projection is recognized: its first condition is
    integer BETWEEN with a BIGINT-column -> INTEGER upper cast; later conditions are boolean
    constants and all results are columns. Thus every row reaches the first condition and no
    other cast can explain the reported error. Unknown shapes stay BUG?.
    """
    from .render import Between, Bin, Case, Cast, Col, Lit
    if not why.startswith("target raised an error") or "a BIGINT value does not fit INTEGER in this conversion" not in why:
        return False
    if query is None or tables is None or query.where is not None or query.limit is not None or len(query.items) != 1:
        return False
    case = query.items[0].e
    if not isinstance(case, Case) or not case.whens:
        return False
    between = case.whens[0][0]
    if not isinstance(between, Between) or not isinstance(between.hi, Cast) or between.hi.t != "INTEGER":
        return False
    high = between.hi.x
    if not isinstance(high, Col) or high.t != "BIGINT":
        return False
    if not all(isinstance(v, Col) for _, v in case.whens) or not isinstance(case.els, Col):
        return False
    if not all(isinstance(c, Lit) and c.t == "BOOLEAN" and c.text.lower() in ("true", "false", "null") for c, _ in case.whens[1:]):
        return False

    def integer(e, row):
        if isinstance(e, Col) and e.t in ("INTEGER", "BIGINT"):
            text = row[[n for n, _ in subset.COLS].index(e.name)]
            return None if text == "NULL" else int(text)
        if isinstance(e, Lit) and e.t in ("INTEGER", "BIGINT"):
            return None if e.text == "NULL" else int(e.text)
        if isinstance(e, Bin) and e.op == "+" and e.t == "INTEGER":
            l, r = integer(e.l, row), integer(e.r, row)
            if l is None or r is None: return None
            value = l + r
            if -(2**31) <= value < 2**31: return value
        raise ValueError("unproved integer expression or overflow")

    witness = False
    try:
        for row in tables[query.frm.name]:
            x, lo, hi = integer(between.x, row), integer(between.lo, row), integer(high, row)
            if hi is None or -(2**31) <= hi < 2**31: continue
            if x is None or lo is None:
                witness = True       # NULL lower comparison: §9b still evaluates the upper cast
            elif x >= lo:
                return False         # TRUE lower comparison: DuckDB should also reach the cast
            # FALSE: a correct executor skips the upper cast; this row cannot justify the class.
    except (ValueError, KeyError, IndexError):
        return False
    return witness


def expression_nodes(node):
    """Walk every expression, including CASE pairs and nested projections."""
    from dataclasses import fields, is_dataclass
    if isinstance(node, tuple):
        for child in node: yield from expression_nodes(child)
    elif is_dataclass(node):
        yield node
        for field in fields(node): yield from expression_nodes(getattr(node, field.name))


def minimum_negation_ast_class(why, query):
    """The shrunk AST has no source positions from the original query.

    Require the exact unary error/type and a minimum-literal negation. Reject
    competing nonliteral negations of that type: their overflow is unproved.
    Any chain around a negative minimum reaches an overflowing inner negation.
    """
    from .render import Neg, Lit
    if not why.startswith("target raised an error"): return None
    match = re.search(r"integer overflow in a unary minus: the result does not fit (INTEGER|BIGINT)\.", why)
    if not match: return None
    typ = match[1]
    minimum = "-2147483648" if typ == "INTEGER" else "-9223372036854775808"
    found = False
    for node in expression_nodes(query):
        if not isinstance(node, Neg) or node.t != typ: continue
        leaf = node.x
        while isinstance(leaf, Neg) and leaf.t == typ: leaf = leaf.x
        if not isinstance(leaf, Lit) or leaf.t != typ or not re.fullmatch(r"-?[0-9]+", leaf.text):
            return None
        if leaf.text == minimum: found = True
    if found:
        return "listed: negating a minimum signed literal overflows its value-based INTEGER/BIGINT type; DuckDB folds a wider literal (dialect 8b.10, 9)"
    return None


def decimal_remainder_product_class(why, query, tables):
    """Certify the replay's tight % -> Decimal64 multiply with actual rows.

    Keep unknown shapes/filters and other multiplications as candidate bugs.
    A mere '%' anywhere in SQL must never excuse an unrelated overflow.
    """
    from decimal import Decimal, localcontext
    from .render import Bin, Col, Lit, Cast
    if not why.startswith("target raised an error") or "decimal overflow in `*`: the result does not fit DECIMAL(18,7)." not in why:
        return None
    if query is None or tables is None or query.where is not None or query.limit is not None:
        return None
    products = [n for n in expression_nodes(query) if isinstance(n, Bin) and n.op == "*"]
    expected = Bin("DECIMAL(25,5)", "%", Col("DECIMAL(25,5)", "w"), Lit("DECIMAL(10,2)", "99999999.99"))
    # Generator annotations are operand families, not bound result types.
    if len(products) != 1: return None
    product = products[0]
    if product.l != expected or product.r != Cast("DECIMAL(10,2)", Col("DECIMAL(10,2)", "d")):
        return None
    # Only an unguarded projection (optionally a literal comparison) proves
    # every row reaches this multiplication. Do not excuse a CASE/AND guard bug.
    if len(query.items) != 1: return None
    root = query.items[0].e
    if root != product and not (isinstance(root, Bin) and root.op in ("=", "<>", "<", "<=", ">", ">=")
                                and root.r == product and isinstance(root.l, Lit)):
        return None
    names = [n for n, _ in subset.COLS]
    with localcontext() as ctx:
        ctx.prec = 80
        for row in tables[query.frm.name]:
            w, d = row[names.index("w")], row[names.index("d")]
            if "NULL" in (w, d): continue
            value = (Decimal(w) % Decimal("99999999.99")) * Decimal(d)
            if abs(value) >= Decimal(10) ** 11:
                return "listed: the tight decimal % result makes multiplication overflow DECIMAL(18,7) (decimal-rules 3, 9; dialect 9)"
    return None


def shrunk_class(why, sql, query=None, tables=None):
    """The class of a difference from the SHRUNK query (what is left once everything that does not matter is gone). `listed:` classes are differences that
    docs/dialect.md section 9 or docs/decimal-rules.md section 9 lists and that are consequences of one rule of Nata: errors are raised only for the rows an
    operand or a branch is evaluated for. Anything else is a candidate bug (class `BUG?`) and fails the run."""
    if null_between_cast_difference(why, query, tables):
        return "listed: BETWEEN with a NULL lower comparison still evaluates its upper-bound cast in a CASE condition; DuckDB skips it (dialect 9b)"
    minimum = minimum_negation_ast_class(why, query)
    if minimum: return minimum
    decimal = decimal_remainder_product_class(why, query, tables)
    if decimal: return decimal
    up = sql.upper()
    m = re.match(r"row \d+ differs: oracle (.*) vs target (.*)$", why)
    if m and m.group(1) != m.group(2) and re.sub(r"'(-?)0\.", r"'\1.", m.group(1)) == re.sub(r"'(-?)0\.", r"'\1.", m.group(2)):
        # the same text but for the zero before the point: a DECIMAL(p,p) (the precision of a literal below one, or of a decimal % result) prints without it
        return "listed: a DECIMAL(p,p) prints without its leading zero, and a literal below one or the tight result type of % makes one (dialect 9, decimal-rules 9)"
    if why.startswith("target raised an error"):
        if "NULL" in up:
            return "listed: Nata raised an error in an operand next to a NULL; DuckDB did not evaluate it (dialect 9b)"
        return "BUG? nata-error: " + classify(why)
    if why.startswith("oracle raised an error"):
        if "[Nata answered with 0 rows]" in why and constant_select_list(sql):
            return "listed: a select list of literals that fails (a CAST, an overflow) is an error in DuckDB over its in-memory tables even when no row is selected (planning folds it); Nata raises it only for a selected row (dialect 9b)"
        if " OR " in up or " AND " in up:
            return "listed: DuckDB evaluates every operand of AND/OR in a projection; Nata only the rows the earlier operands left undecided (dialect 9b)"
        if "CASE" in up:
            return "listed: DuckDB evaluates a constant expression of a CASE condition when planning; Nata only for the rows that reach it (dialect 9b)"
        if "LIMIT" in up:
            return "listed: DuckDB evaluated rows beyond the limit (optimizer off); Nata stops at offset + n (dialect 9b)"
        return "BUG? oracle-error-nata-answered"
    return "BUG? " + classify(why)


def cmd_run(args):
    faults = [x for x in (args.fault or "").split(",") if x]
    stats = {"queries": 0, "oracle_errors": 0, "skipped": 0, "diffs": 0, "rows": 0, "optimizer_dependent": 0, "listed": 0, "classes": Counter(), "skip_reasons": Counter()}
    found, t0 = [], time.time()
    batches = max(1, args.cases // args.per_db)
    with ThreadPoolExecutor(args.jobs) as ex:
        for batch_index, (st, diffs, case) in enumerate(ex.map(lambda b: run_batch(args, args.seed, b, faults), range(batches))):
            merge(stats, st); found += [(d, case) for d in diffs]
            if (batch_index + 1) % 50 == 0:
                print(f"subset progress queries={stats['queries']} differences={stats['diffs']}", flush=True)
    print(f"seed {args.seed}  queries {stats['queries']}  oracle-errors {stats['oracle_errors']} ({100 * stats['oracle_errors'] // max(1, stats['queries'])}%)  "
          f"rows-compared {stats['rows']}  optimizer-dependent-in-duckdb {stats['optimizer_dependent']}  nata-not-supported {stats['skipped']}  "
          f"listed-by-message {stats['listed']}  differences-to-classify {stats['diffs']}  [{time.time() - t0:.0f}s]")
    for k, v in sorted(stats["classes"].items()): print(f"  class (by message) {k}: {v}")
    for k, v in stats["skip_reasons"].most_common(15): print(f"  not supported: {v:5}  {k}")
    os.makedirs(args.out, exist_ok=True)
    final, bugs = Counter(), []

    def work(item):
        (seed, batch, qid, why, sql), (tables, queries) = item
        sh = shrink(queries[qid], tables, checker(args, faults, reason_class(why)), max_checks=args.shrink_checks)
        # Shrinking can change the failing operation (decimal multiplication to
        # integer negation in seed 22014202119). Classify its current error.
        setup = subset.setup_sql(sh[1])
        oracle = DuckDB(args.duckdb)
        target = target_for(args, faults)
        sql = sh[0].sql(NO)
        ro = oracle.run(setup, [(0, sql)])[0]
        rt = target.run(setup, [(0, sh[0].sql(frozenset(faults)))])[0]
        if args.target_cmd and ro.ok and rt.ok:
            exact_aggregate_oracle(sh[0], sh[1], ro)
        why = compare(ro, rt, sh[0].ordered)
        if why and why.startswith("oracle raised an error") and rt.ok and not rt.rows:
            why += " [Nata answered with 0 rows]"
        return item, sh, why
    with ThreadPoolExecutor(args.jobs) as ex:
        results = list(ex.map(work, found[:args.shrink]))
    for (d, _), sh, why in results:
        seed, batch, qid, original_why, sql = d
        cls = shrunk_class(why, sh[0].sql(NO), sh[0], sh[1])
        final[cls] += 1
        path = os.path.join(args.out, f"subset-seed{seed}-b{batch}-q{qid}.sql")
        with open(path, "w") as f:
            f.write(f"-- replay: python3 -I fuzz.py replay-subset --seed {seed} --batch {batch} --query {qid} --target-cmd 'python3 -I adapters/nata_adapter.py'\n-- class: {cls}\n-- difference: {why}\n")
            for s_ in subset.setup_sql(sh[1]): f.write(s_ + ";\n")
            f.write(f"-- shrunk query:\n{sh[0].sql(NO)};\n")
        if cls.startswith("BUG?"): bugs.append((d, sh, path))
        elif final[cls] <= 2: print(f"  e.g. [{cls[:60]}...] seed={seed} batch={batch} query={qid}: {sh[0].sql(NO)[:200]}")
    for k, v in sorted(final.items()): print(f"  FINAL CLASS {k}: {v}")
    for (seed, batch, qid, why, sql), sh, path in bugs:
        print(f"BUG? seed={seed} batch={batch} query={qid}: {why[:200]}\n     {sql[:300]}\n     shrunk: {sh[0].sql(NO)[:300]}\n     -> {path}")
    unshrunk = len(found) - len(results)
    if unshrunk > 0: print(f"  {unshrunk} more differences were not shrunk (--shrink {args.shrink}); treated as BUG?")
    return 1 if (bugs or unshrunk > 0) else 0


def cmd_replay(args):
    tables, queries = build(args.seed, args.batch, args.per_db, args.max_rows)
    setup = subset.setup_sql(tables)
    idx = range(len(queries)) if args.query < 0 else [args.query]
    faults = [x for x in (args.fault or "").split(",") if x]
    F = frozenset(faults)
    extra = [e for f in faults for e in (FAULTS[f][1] or [])]
    oracle = DuckDB(args.duckdb); target = Command(shlex.split(args.target_cmd)) if args.target_cmd else DuckDB(args.duckdb, session_extra=extra)
    for i in idx:
        sql = queries[i].sql(NO)
        ro, rt = oracle.run(setup, [(0, sql)])[0], target.run(setup, [(0, queries[i].sql(F))])[0]
        if args.target_cmd and ro.ok and rt.ok: exact_aggregate_oracle(queries[i], tables, ro)
        why = compare(ro, rt, queries[i].ordered)
        print(f"query {i}: {sql}\n  oracle: {ro.rows if ro.ok else 'ERROR ' + ro.error}\n  target: {rt.rows if rt.ok else 'ERROR ' + rt.error}\n  -> {why or 'same'}")
    return 0


def cmd_prove(args):
    bad = 0
    ctl_q = ctl_d = 0
    with ThreadPoolExecutor(args.jobs) as ex:
        for st, diffs, _ in ex.map(lambda b: run_batch(args, args.seed, b, []), range(max(1, args.control // args.per_db))):
            ctl_q += st["queries"]; ctl_d += st["diffs"]
    print(f"control (faithful rendering vs oracle): {ctl_q} queries, {ctl_d} differences -> {'ok' if ctl_d == 0 else 'FAIL'}")
    bad += ctl_d != 0
    maxb = max(1, args.cases // args.per_db)

    def one(name):
        seen = 0
        for b in range(maxb):
            st, diffs, case = run_batch(args, args.seed, b, [name])
            if diffs:
                return name, seen + diffs[0][2] + 1, diffs[0][4][:100]
            seen += st["queries"]
        return name, None, ""
    with ThreadPoolExecutor(args.jobs) as ex:
        res = list(ex.map(one, SUBSET_FAULTS))
    for name, n, sql in res:
        if n is None: print(f"SURVIVED {name:34} not caught in {args.cases} queries"); bad += 1
        else: print(f"caught   {name:34} after {n:5} queries: {sql}")
    print(f"{sum(1 for r in res if r[1])} of {len(SUBSET_FAULTS)} subset faults caught; control {'clean' if ctl_d == 0 else 'NOT clean'}")
    return 1 if bad else 0


def aggregate_type_class(fn, source, actual, reference):
    """Metadata differences are narrowly typed; wrong Nata types never qualify."""
    if fn == "sum" and source in ("TINYINT", "SMALLINT", "INTEGER") and actual == "BIGINT" and reference == "HUGEINT":
        return "listed: integer SUM returns BIGINT (dialect 10)"
    if fn == "sum" and source == "BIGINT" and actual == "DECIMAL(38,0)" and reference == "HUGEINT":
        return "listed: BIGINT SUM returns DECIMAL(38,0) (dialect 10)"
    return None


def exact_aggregate_oracle(query, tables, result):
    """Replace only direct AVG columns using independent integer/rational arithmetic.

    Generated WHERE/HAVING are safe id/count comparisons. Match groups by their
    complete keys, preserving NULL groups and multiplicities. No value mismatch
    is excused: compare() still checks Nata against this independent answer.
    """
    from decimal import Decimal, localcontext, ROUND_HALF_UP
    from .render import Agg, Col, Bin, Cast
    avgs = [(i, it.e) for i,it in enumerate(query.items) if isinstance(it.e, Agg) and it.e.fn == "avg"]
    if not avgs: return False
    # The shrinker may remove/rewrite key projections, WHERE or AVG arguments.
    # Only certify the exact independently understood AST, never infer it from
    # output positions after a rewrite.
    if tuple(it.e for it in query.items[:len(query.group)]) != query.group: return False
    if any(not isinstance(a.arg, Col) or a.arg.t not in subset.INTS_ALL + subset.DEC_TYPES for _,a in avgs): return False
    if query.where is not None and not (isinstance(query.where, Bin) and query.where.op == ">" and isinstance(query.where.l, Col) and query.where.l.name == "id" and hasattr(query.where.r, "text")):
        return False
    def val(e, row):
        if isinstance(e, Col):
            text = row[[n for n,_ in subset.COLS].index(e.name)]
            if text == "NULL": return None
            if e.t in subset.INTS_ALL + subset.DEC_TYPES: return Decimal(text)
            return text
        if isinstance(e, Cast): return val(e.x, row)
        if isinstance(e, Bin) and e.op == "%": return val(e.l, row) % Decimal(e.r.text)
        raise ValueError("unsupported independent key")
    groups = {}
    rows = tables[query.frm.name]
    for row in rows:
        if query.where and int(row[0]) <= int(query.where.r.text): continue
        key = tuple(val(e, row) for e in query.group)
        groups.setdefault(key, []).append(row)
    if not query.group and not groups: groups[()] = []
    for row in result.rows:
        # Numeric keys are read from DuckDB JSON, textual/date/bool key spelling
        # differs from setup SQL. Match via a separate key-only DuckDB result is
        # unnecessary: AVG oracle supports numeric grouping keys only.
        if any(not (isinstance(e, (Bin, Cast)) or isinstance(e, Col) and e.t in subset.INTS_ALL + subset.DEC_TYPES) for e in query.group):
            return False
        key = tuple(None if x is None else Decimal(str(x)) for x in row[:len(query.group)])
        if key not in groups: return False
        selected = groups[key]
        for i, agg in avgs:
            values = [val(agg.arg, r) for r in selected if val(agg.arg,r) is not None]
            if not values: row[i] = None; continue
            t = agg.arg.t
            p, scale = ({"TINYINT":(3,0),"SMALLINT":(5,0),"INTEGER":(10,0),"BIGINT":(19,0)}.get(t) or tuple(map(int, t[8:-1].split(','))))
            scale = min(scale + 6, 38 - (p-scale))
            with localcontext() as ctx:
                ctx.prec = 100
                row[i] = str((sum(values) / len(values)).quantize(Decimal(1).scaleb(-scale), rounding=ROUND_HALF_UP))
            result.types[i] = f"DECIMAL(38,{scale})"
    return True
