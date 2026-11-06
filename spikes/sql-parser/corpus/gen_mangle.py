"""Corpus (d): the in-subset queries of the test set, mangled by one edit: delete a character, swap two adjacent characters, or insert a character.
`edit` is the character index (in the mangled text) of the first changed character; for a deletion it is the index of the character that followed the
deleted one. Seeded: mangle i of seed S comes from random.Random(S * 7_919 + i).
usage: gen_mangle.py TESTSET.jsonl --seed S --n 10000 --out FILE"""
import argparse, json, random

ALPHABET = list("'\"(),;*+-/%=<>!.:|$?[]{}@#&^~ _\n\t0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ") + ["é", "日", "😀", " "]

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("testset")
    ap.add_argument("--seed", type=int, default=20261106)
    ap.add_argument("--n", type=int, default=10000)
    ap.add_argument("--out", default="mangled.jsonl")
    a = ap.parse_args()
    base = [r for r in map(json.loads, open(a.testset)) if r["expect"] == "accept"]
    with open(a.out, "w") as f:
        for i in range(a.n):
            rng = random.Random(a.seed * 7_919 + i)
            b = rng.choice(base)
            s = b["q"]
            op = rng.choice(["delete", "swap", "insert"])
            if op == "swap":
                cands = [k for k in range(len(s) - 1) if s[k] != s[k + 1]]
                if not cands:
                    op = "delete"
            if op == "delete":
                k = rng.randrange(len(s))
                m = s[:k] + s[k + 1:]
                edit = k
            elif op == "swap":
                k = rng.choice(cands)
                m = s[:k] + s[k + 1] + s[k] + s[k + 2:]
                edit = k
            else:
                k = rng.randrange(len(s) + 1)
                c = rng.choice(ALPHABET)
                m = s[:k] + c + s[k:]
                edit = k
            f.write(json.dumps({"id": i, "base": b["id"], "op": op, "edit": edit, "q": m}, ensure_ascii=False) + "\n")
    print(f"wrote {a.n} mangled queries to {a.out} (seed {a.seed})")
