"""Builds the tables of the results file from the corpora. usage: report.py SECTION [...]   sections: corpus accept literals errors mutants
Everything is seeded; scratch files go to ../target/corpus."""
import collections, json, os, random, re, subprocess, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lib import *

SEED = 20261106
CORE = ("hand", "a-generic", "a-postgres")


def corpora():
    os.makedirs(OUT, exist_ok=True)
    ts = jl(os.path.join(HERE, "testset.jsonl"))
    subprocess.run([sys.executable, "-I", os.path.join(HERE, "gen_fuzz.py"), "--seed", str(SEED), "--trees", "5000", "--out", os.path.join(OUT, "fuzz.jsonl")], check=True, capture_output=True)
    subprocess.run([sys.executable, "-I", os.path.join(HERE, "gen_mangle.py"), os.path.join(HERE, "testset.jsonl"), "--seed", str(SEED), "--n", "10000", "--out", os.path.join(OUT, "mangled.jsonl")], check=True, capture_output=True)
    env = {k: v for k, v in os.environ.items() if k != "AI_AGENT"}
    subprocess.run(["duckdb", "-c", f"INSTALL tpch; LOAD tpch; COPY (SELECT query_nr AS id, query AS q FROM tpch_queries() ORDER BY 1) TO '{OUT}/tpch.jsonl' (FORMAT JSON);"], check=True, env=env, capture_output=True)
    return ts, jl(os.path.join(OUT, "fuzz.jsonl")), jl(os.path.join(OUT, "mangled.jsonl")), jl(os.path.join(OUT, "tpch.jsonl"))


def duck_ok(r):
    """Did DuckDB's parser accept the text? (A statement it parses but cannot serialize counts as accepted.)"""
    if not r.get("error"):
        return True
    return r.get("error_type") == "not implemented"


def pct(a, b):
    return f"{a} of {b} ({100 * a / b:.1f}%)" if b else "0 of 0"


def table(headers, rows):
    out = ["| " + " | ".join(headers) + " |", "|" + "|".join("---" for _ in headers) + "|"]
    out += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    return "\n".join(out)


def short(q, n=90):
    q = q.replace("\n", "\\n").replace("\t", "\\t")
    return q if len(q) <= n else q[: n - 3] + "..."


# ----------------------------------------------------------------------------- tokens for the position check
TOK = re.compile(r"""'(?:[^']|'')*'?|"(?:[^"]|"")*"?|--[^\n]*|/\*.*?(?:\*/|$)|\s+|[^\W\d]\w*|\d+(?:\.\d*)?(?:[eE][+-]?\d+)?|\.\d+|<=|>=|<>|!=|::|\|\||.""", re.S)


def tokens(s):
    out = []
    for m in TOK.finditer(s):
        t = m.group()
        if t.isspace() or t.startswith("--") or t.startswith("/*"):
            continue
        out.append((m.start(), m.end()))
    return out


def tok_index(toks, p):
    for i, (a, b) in enumerate(toks):
        if p < b:
            return i if p >= a else i  # inside a token, or in the gap before it: that token
    return len(toks)


def err_char_pos(q, e):
    """Character index of the arm's reported position; None when it reported none."""
    if e.get("off") is None:
        return None
    return len(q.encode()[: e["off"]].decode(errors="ignore"))


# ----------------------------------------------------------------------------- section: corpus sizes
def sec_corpus(ts, fz, mg, tp):
    c = collections.Counter(r["expect"] for r in ts)
    print(table(["corpus", "size", "how made", "seed"], [
        ["(a) test set", f"{len(ts)}: {c['accept']} in the subset, {c['refuse']} outside it or invalid, {c['edge']} dialect edge cases", "written by hand from the card (`corpus/testset.py`)", "-"],
        ["(b) TPC-H", f"{len(tp)}", "DuckDB 2.0.0 `tpch_queries()`", "-"],
        ["(c) generated", f"{len(fz)} = {len(fz) // 2} trees x 2 renderings", "sqlfuzz `gen.expr`/`gen.boolean` rejection-sampled to the subset (`corpus/gen_fuzz.py`)", SEED],
        ["(d) mangled", f"{len(mg)}", f"one edit (delete, swap, insert) of the {c['accept']} in-subset queries of (a) (`corpus/gen_mangle.py`)", SEED],
    ]))


# ----------------------------------------------------------------------------- section: acceptance
def classify_disagreement(q, hand, duck_accepts):
    """hand: the hand arm's outcome dict. Returns a class name for a disagreement with DuckDB."""
    if hand["ok"] and not duck_accepts:
        return "hand accepts, DuckDB refuses"
    if not hand["ok"] and duck_accepts:
        return "ours stricter (not supported yet)" if hand["kind"] == "unsupported" else "ours stricter (syntax rule)"
    return None


STD_OPS = {"+", "-", "*", "/", "%", "=", "<>", "!=", "<", "<=", ">", ">=", "~~", "contains", "!~~", "~~*", "!~~*"}


def classify_class(q, a, d, c):
    """Why do the arm and DuckDB disagree? A rule per reason; 'other' is listed."""
    js = json.dumps(d)
    if c.startswith("ours stricter (not supported"):
        return "outside the subset by design: " + re.sub(r"'[^']*'", "'..'", a["msg"])[:40]
    if c == "ours stricter (syntax rule)":
        m = a.get("msg", "")
        if m.startswith("trailing characters after numeric"):
            return "DuckDB laxer: a number followed by letters is a number and a name"
        if m.startswith("LIMIT value"):
            return "DuckDB laxer: the size of a LIMIT is not a parse matter"
        if re.search(r",\s*(\)|FROM\b)", q, re.I):
            return "DuckDB laxer: trailing comma"
        if re.search(r"\bISNULL\b|\bNOTNULL\b", q, re.I):
            return "DuckDB laxer: ISNULL / NOTNULL"
        if re.search(r";\s*[^\s;]", q) and "after the end" in m:
            return "DuckDB laxer: text after a semicolon"
        names = set(re.findall(r'"function_name": "([^"]*)", "schema": "", "filter": null, "order_bys": \{"type": "ORDER_MODIFIER", "orders": \[\]\}, "distinct": false, "is_operator": true', js))
        if any(n not in STD_OPS and not n.isalnum() for n in names):
            return "DuckDB laxer: any run of operator characters is an operator"
        if "DECIMAL(" in q.upper() and ("precision" in m or "scale" in m):
            return "DuckDB laxer: type modifiers are any list"
        if re.search(r'"type_name": "(?!DECIMAL|INTEGER|BIGINT|DOUBLE|VARCHAR|DATE|BOOLEAN|TINYINT|SMALLINT|INT\b|NUMERIC)[^"]*"', js):
            return "DuckDB laxer: any name before a string is a typed literal"
        return "other: " + re.sub(r"'[^']*'", "'..'", m)[:45]
    if c.startswith("hand accepts"):
        em = d.get("error_message", "")
        if "without selection list" in em:
            return "arm laxer: empty select list"
        return "arm laxer: DuckDB says " + em[:40]
    return "?"


def sec_accept(ts, fz, mg, tp):
    arms_ts = bake_run(ts, "testset")
    arms_fz = bake_run(fz, "fuzz")
    arms_mg = bake_run(mg, "mangled")
    arms_tp = bake_run([{"id": r["id"], "q": r["q"]} for r in tp], "tpch")
    res = {}
    # ---- (a)
    rows = []
    mism = []
    for exp in ("accept", "refuse", "edge"):
        sub = [r for r in ts if r["expect"] == exp]
        for arm in CORE:
            if exp == "accept":
                same = sum(1 for r in sub if arms_ts[r["id"]][arm]["ok"] and arms_ts[r["id"]][arm]["ast"] == arms_ts[r["id"]]["hand"].get("ast"))
                acc = sum(1 for r in sub if arms_ts[r["id"]][arm]["ok"])
                rows.append([exp, arm, len(sub), f"{acc}", f"{same}"])
                for r in sub:
                    a = arms_ts[r["id"]]
                    if arm != "hand" and (a[arm]["ok"] != a["hand"]["ok"] or a[arm].get("ast") != a["hand"].get("ast")):
                        mism.append((arm, r, a[arm], a["hand"]))
            elif exp == "refuse":
                ref = [r for r in sub if not arms_ts[r["id"]][arm]["ok"]]
                ns = sum(1 for r in ref if arms_ts[r["id"]][arm]["kind"] == "unsupported")
                rows.append([exp, arm, len(sub), f"{len(ref)} refused ({ns} as `not supported yet`)", "-"])
                for r in sub:
                    if arms_ts[r["id"]][arm]["ok"]:
                        mism.append((arm, r, arms_ts[r["id"]][arm], None))
            else:
                acc = sum(1 for r in sub if arms_ts[r["id"]][arm]["ok"])
                agree = sum(1 for r in sub if arms_ts[r["id"]][arm]["ok"] == arms_ts[r["id"]]["hand"]["ok"] and (not acc or arms_ts[r["id"]][arm].get("ast") == arms_ts[r["id"]]["hand"].get("ast")))
                rows.append([exp, arm, len(sub), f"{acc} accepted", f"{agree} same outcome and AST as hand"])
    print("### (a) test set: acceptance, same AST\n")
    print(table(["class", "arm", "queries", "accepted / refused", "same AST as hand"], rows))
    print("\nDifferences from the expectation or from the hand arm (a):\n")
    print(table(["arm", "id", "tag", "query", "arm says", "hand says"], [[m[0], m[1]["id"], m[1]["tag"], "`" + short(m[1]["q"], 70) + "`", short(str(m[2].get("ast") or (m[2].get("kind", "") + ": " + m[2].get("msg", ""))), 80), short(str((m[3] or {}).get("ast") or (m[3] or {}).get("msg", "-")), 80)] for m in mism]))
    # ---- (b) TPC-H
    print("\n### (b) TPC-H, 22 queries\n")
    rows = []
    n_ref = collections.Counter()
    for r in tp:
        a = arms_tp[str(r["id"])]
        h, g, p, raw = a["hand"], a["a-generic"], a["a-postgres"], a["a-generic-raw"]
        for k, v in (("hand", h), ("a-generic", g), ("a-postgres", p)):
            if not v["ok"]:
                n_ref[k] += 1
        rawok = raw.get("msg") == "raw-ok"
        n_ref["raw-parses"] += rawok
        rawpg = None
        rows.append([r["id"], f"{h['kind']} at {h['line']}:{h['col']}: {short(h['msg'], 52)}" if not h["ok"] else "ACCEPTED", f"{g['kind']} at {g['line']}:{g['col']}: {short(g['msg'], 52)}" if not g["ok"] else "ACCEPTED", "parses" if rawok else "FAILS: " + short(raw.get("msg", ""), 40)])
    print(table(["Q", "hand", "sqlparser + layer (generic)", "sqlparser alone (generic)"], rows))
    pg_raw = sum(1 for r in tp if True)
    print(f"\nrefused: hand {n_ref['hand']}/22, A-generic {n_ref['a-generic']}/22, A-postgres {n_ref['a-postgres']}/22; parsed by sqlparser alone (generic): {n_ref['raw-parses']}/22\n")
    # ---- (c)
    print("### (c) generated queries\n")
    rows = []
    byt = collections.defaultdict(dict)
    for r in fz:
        byt[r["tree"]][r["form"]] = arms_fz[str(r["id"])]
    for arm in CORE:
        acc = sum(1 for r in fz if arms_fz[str(r["id"])][arm]["ok"])
        same = sum(1 for r in fz if arms_fz[str(r["id"])][arm]["ok"] and arms_fz[str(r["id"])][arm]["ast"] == arms_fz[str(r["id"])]["hand"].get("ast"))
        both = sum(1 for t, f in byt.items() if f["full"][arm].get("ast") == f["mini"][arm].get("ast") and f["full"][arm]["ok"])
        rows.append([arm, pct(acc, len(fz)), pct(same, len(fz)), pct(both, len(byt))])
    print(table(["arm", "accepted", "same AST as hand", "both renderings of a tree give one AST"], rows))
    diffs = [r for r in fz if arms_fz[str(r["id"])]["a-generic"].get("ast") != arms_fz[str(r["id"])]["hand"].get("ast")]
    print(f"\nA-generic differs from hand on {len(diffs)} queries; the first 4:\n")
    for r in diffs[:4]:
        a = arms_fz[str(r["id"])]
        print(f"- `{short(r['q'], 150)}`\n  - hand: `{short(a['hand']['ast'], 300)}`\n  - generic: `{short(a['a-generic']['ast'], 300)}`")
    # ---- DuckDB agreement
    print("\n### agreement of the accept/reject decision with DuckDB's parser\n")
    out_rows = []
    cls_all = {}
    for name, rows_, arms_ in (("(a) test set", ts, arms_ts), ("(c) generated", fz, arms_fz), ("(d) mangled", mg, arms_mg)):
        dres = duck_run(rows_, name.split()[0].strip("()") + "_" + {"(a)": "testset", "(c)": "fuzz", "(d)": "mangled"}[name.split()[0]])
        cls = collections.Counter()
        ex = collections.defaultdict(list)
        for r, d in zip(rows_, dres):
            dok = duck_ok(d)
            for arm in CORE:
                a = arms_[str(r["id"])][arm]
                c = classify_disagreement(r["q"], a, dok) or "agree"
                cls[(arm, c)] += 1
                if c != "agree":
                    ex[(arm, c)].append((r, a, d))
        cls_all[name] = (cls, ex, rows_, dres)
        for arm in CORE:
            out_rows.append([name, arm, len(rows_)] + [cls[(arm, k)] for k in ("agree", "ours stricter (not supported yet)", "ours stricter (syntax rule)", "hand accepts, DuckDB refuses")])
    print(table(["corpus", "arm", "queries", "agree", "ours stricter: not supported yet", "ours stricter: syntax rule", "arm accepts, DuckDB refuses"], out_rows))
    json.dump({n: {"%s|%s" % k: v for k, v in c[0].items()} for n, c in cls_all.items()}, open(os.path.join(OUT, "agreement.json"), "w"), indent=1)
    # ---- classes of the disagreements
    print("\nClasses of the disagreements (every one examined; the class is a rule on the query text, the hand message and DuckDB's own tree):\n")
    rows = []
    for name, (cls, ex, rows_, dres) in cls_all.items():
        for (arm, c), lst in sorted(ex.items()):
            if arm != "hand" and not c.startswith("hand accepts"):
                continue
            sub = collections.Counter()
            samp = {}
            for r, a, d in lst:
                k = "outside the subset by design (a `not supported yet` refusal)" if c.startswith("ours stricter (not supported") else classify_class(r["q"], a, d, c)
                sub[k] += 1
                samp.setdefault(k, r["q"])
            for k, n in sub.most_common():
                ex_q = samp[k]
                if k.startswith("other:") and name.startswith("(a)"):
                    k = "other (see the list of the test set)"
                rows.append([name, arm, c, k, n, "`" + short(ex_q, 60) + "`"])
    print(table(["corpus", "arm", "disagreement", "reason", "n", "example"], rows))
    json.dump({"ts": arms_ts, "tp": arms_tp}, open(os.path.join(OUT, "accept.json"), "w"))


# ----------------------------------------------------------------------------- section: error positions (d)
def sec_errors(ts, fz, mg, tp):
    arms = bake_run(mg, "mangled")
    dres = {int(j["id"]): j["r"] for j in jl(os.path.join(OUT, "d_mangled.duck.jsonl"))} if os.path.exists(os.path.join(OUT, "d_mangled.duck.jsonl")) else None
    if dres is None:
        dres = dict(enumerate(duck_run(mg, "d_mangled")))
    names = ["hand", "a-generic", "a-generic-nofix", "a-postgres"]
    stat = {n: collections.Counter() for n in names + ["duckdb"]}
    byop = {n: collections.defaultdict(collections.Counter) for n in names + ["duckdb"]}
    panics = 0
    for i, r in enumerate(mg):
        q = r["q"]
        toks = tokens(q)
        et = tok_index(toks, r["edit"])
        for n in names + ["duckdb"]:
            if n == "duckdb":
                d = dres[i]
                if duck_ok(d):
                    continue
                pos = d.get("position")
                prefix = len(strip_prefix(q))
                cp = None
                if pos is not None:
                    st, _ = __import__("duck").strip_explain(q)
                    cp = prefix + len(st.encode()[: int(pos)].decode(errors="ignore"))
                e = {"ok": False, "off": None}
                has = cp is not None
            else:
                e = arms[str(r["id"])][n]
                if e["ok"]:
                    continue
                if e["kind"] == "panic":
                    panics += 1
                    continue
                cp = err_char_pos(q, e)
                has = e.get("line") is not None and e.get("col") is not None
            c = stat[n]
            c["errors"] += 1
            c["with line and column"] += has
            if has and cp is not None:
                d_ = abs(tok_index(toks, cp) - et)
                c["same token"] += d_ == 0
                c["within 1 token"] += d_ <= 1
                c["within 2 tokens"] += d_ <= 2
                c["at end of text"] += cp >= len(q)
                byop[n][r["op"]]["n"] += 1
                byop[n][r["op"]]["hit"] += d_ <= 1
    print("### (iii) error positions on the mangled queries (d)\n")
    rows = []
    for n in names + ["duckdb"]:
        c = stat[n]
        rows.append([n, c["errors"], pct(c["with line and column"], c["errors"]), pct(c["same token"], c["errors"]), pct(c["within 1 token"], c["errors"]), pct(c["within 2 tokens"], c["errors"]), c["at end of text"]])
    print(table(["arm", "queries refused", "with line and column", "position in the edited token", "within 1 token of the edit", "within 2 tokens", "reported at end of text"], rows))
    print(f"\npanics: {panics} (all arms)\n")
    print("Within 1 token of the edit, by kind of edit:\n")
    print(table(["arm", "delete", "swap", "insert"], [[n] + [pct(byop[n][o]["hit"], byop[n][o]["n"]) for o in ("delete", "swap", "insert")] for n in names + ["duckdb"]]))
    # no position at all, by message (sqlparser raw)
    nopos = collections.Counter()
    for r in mg:
        e = arms[str(r["id"])]["a-generic-nofix"]
        if not e["ok"] and e["kind"] != "panic" and e.get("line") is None:
            nopos[re.sub(r"'[^']*'", "'..'", e["msg"])[:60]] += 1
    print("\nsqlparser errors that carry NO line and column (without the end-of-text rule of the conversion layer), by message:\n")
    print(table(["message", "n"], [[k, v] for k, v in nopos.most_common(8)]))
    # sample of messages
    rng = random.Random(SEED)
    both = [(i, r) for i, r in enumerate(mg) if not arms[str(r["id"])]["hand"]["ok"] and not arms[str(r["id"])]["a-generic"]["ok"] and not duck_ok(dres[i])]
    print("\nA sample of 20 refused mangled queries (random, seed %d): the query, the edit, and what each says (line:column, message):\n" % SEED)
    for i, r in rng.sample(both, 20):
        h, a, d = arms[str(r["id"])]["hand"], arms[str(r["id"])]["a-generic"], dres[i]
        print(f"- `{short(r['q'], 100)}` ({r['op']} at character {r['edit']})")
        print(f"  - hand ({h['line']}:{h['col']}): {h['kind']}: {short(h['msg'], 110)}")
        print(f"  - sqlparser ({a['line']}:{a['col']}): {short(a['msg'], 110)}")
        print(f"  - DuckDB (char {d.get('position')}): {short(d.get('error_message', ''), 80)}")


def strip_prefix(q):
    st, k = __import__("duck").strip_explain(q)
    return q[:k]


# ----------------------------------------------------------------------------- section: literals (ii)
LITS = ["0", "1", "0.1", "0.10", "0.0", "00012", "1.", ".5", "123456789012345678901234567890.12", "99999999999999999999999999999999999999", "0.000000000000000000000000000000000001", "1e400",
        "1E-400", "1.5e+3", "2.5E3", "1e0", "4.9e-324", "1.7976931348623157e308", "9007199254740993", "18446744073709551616", "123456789.123456789123456789e-10", "0.30000000000000004", "100000000000000000000.5e-7"]


def sec_literals(ts, fz, mg, tp):
    rows = [{"id": f"l{i}", "q": f"SELECT {lit} AS r FROM 'f.parquet'"} for i, lit in enumerate(LITS)]
    rows += [{"id": f"n{i}", "q": f"SELECT -{lit} AS r FROM 'f.parquet'"} for i, lit in enumerate(LITS)]
    rows += [{"id": "w1", "q": "SELECT * FROM 'f.parquet' WHERE d BETWEEN 0.05 AND 0.070 AND e IN (1.50, 2e1) AND f = CASE WHEN a THEN 0.10 ELSE 1e400 END AND CAST(g AS DECIMAL(10,2)) > 1.10"}]
    arms = bake_run(rows, "literals", mutants=True)
    dres = duck_run(rows, "literals")
    ok = collections.Counter()
    out = []
    for i, r in enumerate(rows):
        a = arms[r["id"]]
        lit = (LITS + LITS)[i] if i < 2 * len(LITS) else None
        d = dres[i]
        dtext = None
        m = re.search(r'"literal":\{"kind":"[A-Z]+","text":"([^"]*)"', json.dumps(d).replace(" ", ""))
        if m:
            dtext = m.group(1)
        row = [f"`{('-' if r['id'][0] == 'n' else '') + lit}`" if lit else "(the 5 literals of a WHERE)"]
        for n in ("hand", "a-generic", "a-postgres", "mut-number-via-f64"):
            if lit is None:
                nums = re.findall(r"\(num ([^)]*)\)", a[n]["ast"])
                good = nums == ["0.05", "0.070", "1.50", "2e1", "0.10", "1e400", "1.10"]
            else:
                good = a[n]["ok"] and f"(num {lit})" in a[n]["ast"] and (r["id"][0] != "n" or f"(neg (num {lit}))" in a[n]["ast"])
            ok[(n, good)] += 1
            row.append("same text" if good else "CHANGED")
        row.append(dtext if dtext is not None else "-")
        out.append(row)
    print("### (ii) literal fidelity: the text of every numeric literal through each arm\n")
    print(table(["literal", "hand", "sqlparser generic", "sqlparser postgres", "mutant: through f64", "DuckDB's serialized literal text"], out))
    print("\n" + ", ".join(f"{n}: {ok[(n, True)]} of {ok[(n, True)] + ok[(n, False)]} kept" for n in ("hand", "a-generic", "a-postgres", "mut-number-via-f64")))


# ----------------------------------------------------------------------------- section: mutants
def sec_mutants(ts, fz, mg, tp):
    sets = {"(a) accepted test set": [r for r in ts if r["expect"] == "accept"], "(c) generated": fz}
    print("### known-bad hand parsers: does the comparison with sqlparser catch each?\n")
    rows = []
    for sname, rs in sets.items():
        arms = bake_run(rs, "mut_" + sname[1], mutants=True)
        for m in ("mut-or-before-and", "mut-lose-not", "mut-number-via-f64"):
            diff = [r for r in rs if arms[str(r["id"])][m].get("ast") != arms[str(r["id"])]["a-postgres"].get("ast")]
            rows.append([sname, m, len(rs), len(diff), "`" + short(diff[0]["q"], 70) + "`" if diff else "-"])
        base = sum(1 for r in rs if arms[str(r["id"])]["hand"].get("ast") != arms[str(r["id"])]["a-postgres"].get("ast"))
        rows.append([sname, "hand (control, the real parser)", len(rs), base, "-"])
    print(table(["corpus", "parser", "queries", "AST differs from sqlparser (postgres)", "first difference"], rows))


# ----------------------------------------------------------------------------- section: robustness
def cell(r):
    c = r["child"]
    if r["crash"]:
        return "**CRASH** (" + r["crash"].replace("crash: ", "") + ")"
    st = c.get("status", "?")
    kind = "ok" if st == "ok" or st.endswith(":raw-ok") else ("refused: limit" if ":Limit:" in st else "refused: " + st.split(":")[1].lower())
    return kind


def sec_robust(ts, fz, mg, tp):
    rb = [json.loads(l) for l in open(os.path.join(OUT, "robust.jsonl"))]
    rn = [json.loads(l) for l in open(os.path.join(OUT, "robust-nostacker.jsonl"))]
    cases = []
    for r in rb:
        if r["case"] not in cases:
            cases.append(r["case"])
    def get(rows, case, arm, kb):
        return next(r for r in rows if r["case"] == case and r["arm"] == arm and r["stack_kb"] == kb)
    print("### robustness: deep, long and huge queries, each in a fresh process, parse on a thread with a 2 MB and an 8 MB stack\n")
    print("`a-generic` is sqlparser with the conversion layer and the depth guard; `a-generic-raw` is the crate alone (parse and drop, no guard, no conversion); "
          "`a-no-stacker` is the same as `a-generic` built without the crate's `recursive-protection` feature. Time and peak memory (VmHWM growth) are from the 8 MB run.\n")
    rows = []
    for c in cases:
        row = [c]
        for rows_, arm in ((rb, "hand"), (rb, "a-generic"), (rn, "a-generic"), (rb, "a-generic-raw")):
            r2, r8 = get(rows_, c, arm, 2048), get(rows_, c, arm, 8192)
            ch = r8["child"]
            t = "" if r8["crash"] else f"; {ch.get('ms', 0):.0f} ms, +{(ch.get('hwm_after_kb', 0) - ch.get('hwm_before_kb', 0)) / 1024:.0f} MB"
            row.append(f"2 MB: {cell(r2)}; 8 MB: {cell(r8)}{t}")
        rows.append(row)
    print(table(["case", "hand", "a-generic (stacker on)", "a-no-stacker", "a-generic-raw (crate alone)"], rows))
    st = collections.defaultdict(dict)
    for fname, label in (("stack.jsonl", "stacker"), ("stack-nostacker.jsonl", "no-stacker")):
        for l in open(os.path.join(OUT, fname)):
            j = json.loads(l)
            st[j["case"]][(j["arm"], label)] = j["min_stack_kb"]
    print("\nSmallest thread stack (KB, from a ladder 64, 96, 128, ... 65536) at which a 1,000-deep case completes without a crash:\n")
    cols = [("hand", "stacker"), ("a-generic", "stacker"), ("a-generic-raw", "stacker"), ("a-generic", "no-stacker"), ("a-generic-raw", "no-stacker")]
    print(table(["case (1,000 deep)", "hand", "a-generic, stacker", "crate alone, stacker", "a-generic, no stacker", "crate alone, no stacker"],
                [[c] + [st[c].get(k, "-") for k in cols] for c in ("parens-1000", "cases-1000", "nots-1000", "negs-1000", "ands-1000", "sums-1000")]))


if __name__ == "__main__":
    ts, fz, mg, tp = corpora()
    for s in sys.argv[1:]:
        print(f"\n<!-- section {s} -->\n")
        {"corpus": sec_corpus, "accept": sec_accept, "errors": sec_errors, "literals": sec_literals, "mutants": sec_mutants, "robust": sec_robust}[s](ts, fz, mg, tp)
