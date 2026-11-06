#!/usr/bin/env python3
"""R16 diagnostic only: SF10-shaped monotonic BIGINT, not original TPC-H data.
Run a release nata binary, with defaults and MALLOC_ARENA_MAX=1 as a control.
No threshold/production allocator is changed. Writes data only to --work.
"""
import argparse
import os
from pathlib import Path
import subprocess

ap=argparse.ArgumentParser(description=__doc__)
ap.add_argument('--nata',type=Path,required=True)
ap.add_argument('--work',type=Path,required=True)
a=ap.parse_args();a.work.mkdir(parents=True,exist_ok=True)
file=a.work.resolve()/'narrow.parquet';binary=str(a.nata.resolve())
env=dict(os.environ);env.pop('AI_AGENT',None);env.pop('MALLOC_ARENA_MAX',None)
if not file.exists():
    subprocess.run(['duckdb','-no-agent','-c',f"COPY (SELECT i::BIGINT AS id FROM range(59986052) t(i)) TO '{file}' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);"],env=env,check=True)
for arena in [False,True,False,True]:
    runenv=dict(env)
    if arena:runenv['MALLOC_ARENA_MAX']='1'
    for mode in ['sql','scan']:
        cmd=[binary,'sql','--csv','--report','--threads','8','-c',f"SELECT id FROM '{file}'"] if mode=='sql' else [binary,'scan',str(file),'--columns','id','--csv','--threads','8']
        p=subprocess.run(cmd,env=runenv,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,text=True,check=True)
        print('arena_max_1='+str(arena),mode,p.stderr.strip(),flush=True)
