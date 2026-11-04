"""DuckDB CLI batching helpers (one process runs many statements; output order is kept with stdbuf)."""
import csv
import io
import re
import subprocess
from concurrent.futures import ThreadPoolExecutor

DUCK = ["stdbuf", "-o0", "env", "-u", "AI_AGENT", "duckdb", "-csv", "-noheader"]
ERR_RE = re.compile(r"^[A-Za-z ]+ Error:")
WORKERS = 2


def run_script(script):
    p = subprocess.run(DUCK, input=script, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False)
    return p.stdout


def parse_marked(out):
    """-> {jobid: (data_lines, error_lines)}; every job starts with a '@@<id>' line."""
    res = {}
    cur = None
    for line in out.split("\n"):
        if line.startswith("@@"):
            cur = line[2:]
            res[cur] = ([], [])
        elif cur is not None:
            if ERR_RE.match(line):
                res[cur][1].append(line)
            elif res[cur][1]:  # continuation of an error message
                res[cur][1].append(line)
            elif line != "":
                res[cur][0].append(line)
    return res


def run_jobs(jobs, workers=WORKERS, prelude=""):
    """jobs: list of (jobid:str, sql:str). Returns {jobid: (data_lines, error_lines)}."""
    if not jobs:
        return {}
    bins = [[] for _ in range(workers)]
    for i, j in enumerate(jobs):
        bins[i % workers].append(j)

    def work(b):
        if not b:
            return {}
        script = "SET threads=1;\n" + prelude + "".join(f"SELECT '@@{jid}';\n{sql.rstrip().rstrip(';')};\n" for jid, sql in b)
        return parse_marked(run_script(script))

    out = {}
    with ThreadPoolExecutor(workers) as ex:
        for r in ex.map(work, bins):
            out.update(r)
    missing = [j[0] for j in jobs if j[0] not in out]
    assert not missing, f"duckdb lost jobs: {missing[:5]}"
    return out


def csv_row(line):
    return next(csv.reader(io.StringIO(line)))


TYPE_RE = re.compile(r"^DECIMAL\((\d+),(\d+)\)$")


def parse_type(s):
    m = TYPE_RE.match(s)
    if m:
        return (int(m.group(1)), int(m.group(2)))
    return s  # e.g. 'DOUBLE'
