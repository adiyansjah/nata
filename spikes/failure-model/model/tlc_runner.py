#!/usr/bin/env python3
"""Run TLC on one configuration and classify the outcome. Shared by run_model.py and mutate_model.py.

Outcomes (Result.status):
  PASS       "Model checking completed. No error has been found."
  VIOLATED   an invariant or temporal property is violated (Result.what names it)
  ERROR      anything else (parse error, evaluation error, out of memory, crash): never counts as evidence
  TIMEOUT    the time limit was reached before TLC finished: never counts as evidence
"""
import os, re, signal, subprocess, sys, time
from dataclasses import dataclass

@dataclass
class Result:
    status: str
    what: str = ""          # violated invariant / property, or the first error line
    generated: int = 0
    distinct: int = 0
    seconds: float = 0.0
    out: str = ""           # path of the TLC output file

RE_INV = re.compile(r"Error: Invariant (\S+) is violated")
RE_PROP = re.compile(r"Error: (?:Temporal propert(?:y|ies)|Action propert(?:y|ies)) (?:\S+ )?(?:were|was|is) violated")
RE_STATES = re.compile(r"(\d+) states generated, (\d+) distinct states found")
RE_ERR = re.compile(r"^(Error: .*|.*Exception.*|.*OutOfMemory.*)$", re.M)

def classify(text, rc):
    gen = dist = 0
    for m in RE_STATES.finditer(text):
        gen, dist = int(m.group(1)), int(m.group(2))
    m = RE_INV.search(text)
    if m:
        return "VIOLATED", m.group(1), gen, dist
    if RE_PROP.search(text):
        return "VIOLATED", "temporal property", gen, dist
    if "Model checking completed. No error has been found." in text and rc == 0:
        return "PASS", "", gen, dist
    m = RE_ERR.search(text)
    return "ERROR", (m.group(1)[:200] if m else f"exit status {rc}, no recognised TLC output"), gen, dist

def run_tlc(java, jar, workdir, cfg, out, workers, timeout, heap_mb, module="MC"):
    """workdir holds MC.tla, IcebergCommit.tla and the cfg directory. Never touches the real model."""
    meta = os.path.join(workdir, "states-" + re.sub(r"[^A-Za-z0-9_.-]", "_", os.path.basename(cfg)))
    cmd = [java, "-XX:+UseParallelGC", f"-Xmx{heap_mb}m", "-cp", jar, "tlc2.TLC",
           "-config", cfg, "-workers", str(workers), "-deadlock", "-cleanup", "-metadir", meta, module]
    t0 = time.time()
    with open(out, "w") as fh:
        fh.write("$ " + " ".join(cmd) + "\n")
        fh.flush()
        p = subprocess.Popen(cmd, cwd=workdir, stdout=fh, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            rc = p.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()
            return Result("TIMEOUT", f"no result after {timeout}s", 0, 0, time.time() - t0, out)
    text = open(out, errors="replace").read()
    status, what, gen, dist = classify(text, rc)
    return Result(status, what, gen, dist, time.time() - t0, out)

def sany(java, jar, workdir, module="MC"):
    """Parse check (SANY). Returns (ok, text)."""
    p = subprocess.run([java, "-cp", jar, "tla2sany.SANY", module + ".tla"], cwd=workdir,
                       capture_output=True, text=True, timeout=120)
    text = p.stdout + p.stderr
    bad = (re.search(r"Fatal errors|\*+ ?Errors|Parse Error|Semantic error|Abort|Unknown operator|Multiply-defined", text, re.I)
           or p.returncode != 0 or f"Semantic processing of module {module}" not in text)
    return (not bad), text
