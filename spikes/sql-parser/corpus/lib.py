import json, os, subprocess, sys
HERE = os.path.dirname(os.path.abspath(__file__))
SPIKE = os.path.dirname(HERE)
BAKE = os.path.join(SPIKE, "target", "release", "bake")
OUT = os.path.join(SPIKE, "target", "corpus")
sys.path.insert(0, HERE)


def jl(path):
    return [json.loads(l) for l in open(path, encoding="utf-8")]


def export_rs(rows, path, key="q"):
    with open(path, "w", encoding="utf-8", newline="") as f:
        for r in rows:
            f.write(f"{r['id']}\x1f{r[key]}\x1e")


def bake_run(rows, name, mutants=False):
    os.makedirs(OUT, exist_ok=True)
    rs, outp = os.path.join(OUT, name + ".rs"), os.path.join(OUT, name + ".arms.jsonl")
    export_rs(rows, rs)
    subprocess.run([BAKE, "run", rs, outp] + (["--mutants"] if mutants else []), check=True)
    return {str(j["id"]): j["arms"] for j in jl(outp)}


def duck_run(rows, name):
    import duck
    outp = os.path.join(OUT, name + ".duck.jsonl")
    duck.run([(i, r["q"]) for i, r in enumerate(rows)], outp)
    res = {}
    for j in jl(outp):
        res[j["id"]] = j["r"]
    return [res[i] for i in range(len(rows))]
