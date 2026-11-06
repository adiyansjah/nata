"""DuckDB as the parse oracle: `json_serialize_sql` parses without binding (no file needs to exist). One duckdb process for a whole batch.
usage: duck.py QUERIES.jsonl OUT.jsonl   (QUERIES: {"id": n, "q": "..."} per line; OUT: {"id": n, "r": <json_serialize_sql result as JSON text>})"""
import json, os, re, subprocess, sys, tempfile

EXPLAIN = re.compile(r"^\s*EXPLAIN(\s+ANALYZE)?\s+", re.I)


def strip_explain(q):
    """DuckDB cannot serialize EXPLAIN; the SELECT after it is what we compare."""
    m = EXPLAIN.match(q)
    return (q[m.end():], m.end()) if m else (q, 0)


def run(rows, outpath):
    """rows: list of (id, query). Writes outpath as JSON lines {"id", "r"}."""
    with tempfile.TemporaryDirectory() as d:
        qp = os.path.join(d, "q.jsonl")
        with open(qp, "w") as f:
            for i, q in rows:
                s, _ = strip_explain(q)
                f.write(json.dumps({"id": i, "q": s}) + "\n")
        sql = (f"COPY (SELECT id, json_serialize_sql(q) AS r FROM read_json('{qp}', columns={{id:'BIGINT', q:'VARCHAR'}}) ORDER BY id) "
               f"TO '{outpath}' (FORMAT JSON);")
        env = {k: v for k, v in os.environ.items() if k != "AI_AGENT"}
        subprocess.run(["duckdb", "-c", sql], check=True, env=env, capture_output=True)


if __name__ == "__main__":
    rows = [(j["id"], j["q"]) for j in map(json.loads, open(sys.argv[1]))]
    run(rows, sys.argv[2])
