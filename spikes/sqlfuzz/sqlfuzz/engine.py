"""Engines. `DuckDB` is the oracle (and, with faults or settings changed, the known-bad targets). `Command` is the adapter for a real target
(Nata later): `CMD in.json out.json`, see README. Both return {case_id: Result}."""
import json, os, re, subprocess, tempfile
from dataclasses import dataclass, field

MARK = re.compile(r'^\[\{"q":"QID:(\d+):([TR])"\}\]$')


@dataclass
class Result:
    ok: bool
    cols: list = field(default_factory=list)     # column names
    types: list = field(default_factory=list)    # column types as the engine names them ("" if unknown)
    rows: list = field(default_factory=list)     # list of lists of JSON values
    error: str = ""


def duckdb_env():
    e = dict(os.environ)
    e.pop("AI_AGENT", None)      # agent mode changes the CLI's output
    return e


class DuckDB:
    name = "duckdb"

    def __init__(self, binary="duckdb", timeout=120, session_extra=()):
        self.binary, self.timeout, self.session_extra = binary, timeout, list(session_extra)

    def run(self, setup, cases):
        """setup: list of SQL statements; cases: list of (id, sql). A case that errors yields Result(ok=False, error=...)."""
        lines = [s.rstrip(";") + ";" for s in setup] + [s.rstrip(";") + ";" for s in self.session_extra]
        for cid, sql in cases:
            sql = sql.rstrip(";")
            lines.append(f"SELECT 'QID:{cid}:T' AS q;")
            lines.append(f"DESCRIBE {sql};")
            lines.append(f"SELECT 'QID:{cid}:R' AS q;")
            lines.append(sql + ";")
        script = "\n".join(lines) + "\n"
        try:
            p = subprocess.run([self.binary, "-json"], input=script, capture_output=True, text=True, timeout=self.timeout, env=duckdb_env())
        except subprocess.TimeoutExpired:
            return {cid: Result(False, error="timeout") for cid, _ in cases}
        chunks, cur = {}, None
        for ln in p.stdout.split("\n"):
            m = MARK.match(ln)
            if m: cur = (int(m.group(1)), m.group(2)); chunks[cur] = []
            elif cur is not None: chunks[cur].append(ln)
        out = {}
        for cid, _ in cases:
            t, r = chunks.get((cid, "T")), chunks.get((cid, "R"))
            if not t or not r or not "".join(r).strip():
                out[cid] = Result(False, error=self._err(p.stderr, cid) or "error")
                continue
            try:
                desc = json.loads("\n".join(t))
                rows = json.loads("\n".join(r))
            except json.JSONDecodeError as e:
                out[cid] = Result(False, error=f"unparseable output: {e}")
                continue
            cols = [d["column_name"] for d in desc]
            out[cid] = Result(True, cols, [d["column_type"] for d in desc], [list(row.values()) for row in rows])
        return out

    @staticmethod
    def _err(stderr, cid):
        return stderr.strip().split("\n")[0][:200] if stderr.strip() else ""


class Command:
    """A target engine behind a command: `CMD <in.json> <out.json>`. in.json = {"setup": [sql...], "cases": [{"id": n, "sql": "..."}]};
    out.json = {"results": {"<id>": {"cols": [...], "types": [...], "rows": [[...]]} | {"error": "message"}}}. A target that does not support
    something must answer {"error": "not supported yet: <what>"}; the fuzzer counts those as coverage gaps, not as differences."""
    name = "command"

    def __init__(self, cmd, timeout=600): self.cmd, self.timeout = cmd, timeout

    def run(self, setup, cases):
        with tempfile.TemporaryDirectory() as d:
            fin, fout = os.path.join(d, "in.json"), os.path.join(d, "out.json")
            json.dump({"setup": setup, "cases": [{"id": c, "sql": s} for c, s in cases]}, open(fin, "w"))
            try:
                subprocess.run(self.cmd + [fin, fout], check=True, timeout=self.timeout, capture_output=True)
                res = json.load(open(fout))["results"]
            except Exception as e:      # a crash, a hang or a malformed answer is a failure of the target, reported per case
                return {cid: Result(False, error=f"target failed: {type(e).__name__}: {str(e)[:150]}") for cid, _ in cases}
        out = {}
        for cid, _ in cases:
            r = res.get(str(cid))
            if r is None: out[cid] = Result(False, error="no answer")
            elif "error" in r: out[cid] = Result(False, error=r["error"])
            else: out[cid] = Result(True, r.get("cols", []), r.get("types", []), r["rows"])
        return out
