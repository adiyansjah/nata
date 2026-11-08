#!/usr/bin/env python3
"""Exact Nata/before versus Nata/after CSV multiset check for the two numeric workloads."""
import filecmp, importlib.util, json, os, pathlib, subprocess, sys
before,after,data,out=map(pathlib.Path,sys.argv[1:])
out.mkdir(parents=True,exist_ok=True)
sys.path.insert(0,str(pathlib.Path('scripts').resolve()))
spec=importlib.util.spec_from_file_location('heavy',pathlib.Path('scripts/bench-aggregate-perf1-heavy.py'))
h=importlib.util.module_from_spec(spec);spec.loader.exec_module(h)
env=dict(os.environ,TMPDIR=str(out.resolve()),LC_ALL='C');env.pop('AI_AGENT',None)
records=[]
for query in h.QUERIES:
    sql=pathlib.Path('check/queries/aggregate',query+'.sql').read_text().replace('{data}',str(data.resolve()).replace("'","''"))
    for threads in (1,8):
        files=[]
        for label,binary in [('before',before),('after',after)]:
            output=out/(label+'.csv');ordered=out/(label+'-sorted.csv')
            result=h.invoke(h.command(binary.resolve(),sql,threads,8<<30,out.resolve()),env,output)
            assert result['exit']==0,(query,threads,label,result)
            subprocess.run(['sort','--parallel=1','-o',str(ordered),str(output)],env=env,check=True)
            files.append(ordered)
            output.unlink()
        assert filecmp.cmp(*files,shallow=False),(query,threads,'RESULT CHANGED')
        records.append(dict(query=query,threads=threads,exact_csv_multiset='identical',sha256=h.digest(files[0])))
        for f in files:f.unlink()
        (out/'exact-gates.json').write_text(json.dumps(records,indent=2)+'\n')
        print(query,threads,'exact CSV multiset identical',flush=True)
