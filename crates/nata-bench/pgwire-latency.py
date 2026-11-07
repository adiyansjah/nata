#!/usr/bin/env python3
"""Paired one-change TCP_NODELAY control; all other response batching is identical."""
import importlib.util
import json
from pathlib import Path
import statistics
import sys
import time
spec=importlib.util.spec_from_file_location('bench', Path(__file__).with_name('pgwire-bench.py'))
b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
on, off, work, out=sys.argv[1:]
work=Path(work);work.mkdir(parents=True,exist_ok=True)
servers=[b.Server(binary,work,1,1) for binary in [on,off]]
clients=[b.Client(s.port) for s in servers]
samples=[]
try:
    for round_ in range(5):
        for i in ([0,1] if round_%2==0 else [1,0]):
            start=time.perf_counter()
            for _ in range(100):
                assert clients[i].query('SELECT 1',True)['values']==[['1']]
            samples.append(dict(round=round_,nodelay=not i,ms=(time.perf_counter()-start)*10))
finally:
    for c in clients:c.close()
    for s in servers:s.close()
result={'samples':samples,'median_ms':{str(on):statistics.median(s['ms'] for s in samples if s['nodelay']==on) for on in [True,False]}}
Path(out).write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result),flush=True)
