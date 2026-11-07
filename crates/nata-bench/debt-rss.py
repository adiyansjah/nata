#!/usr/bin/env python3
"""R16 paired RSS and heap attribution on the original SF10 projection."""
import importlib.util,json,os,re,subprocess,sys
from pathlib import Path
import pyarrow.parquet as pq
fixed,file,work,out=sys.argv[1:];fixed=Path(fixed).resolve();file=Path(file).resolve();work=Path(work);out=Path(out)
work.mkdir(parents=True,exist_ok=True);out.mkdir(parents=True,exist_ok=True)
# Private known-bad control: restore precisely the unused all_rows allocation.
spec=importlib.util.spec_from_file_location('mut','crates/nata-exec/mutate.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
base=Path(m.stage(str(work/'control')));p=base/'crates/nata-exec/src/exec.rs';s=p.read_text();assert s.count('let mut all = None;')==1;p.write_text(s.replace('let mut all = None;','let mut all = Some(ev.all_rows());'))
env=dict(os.environ,CARGO_TARGET_DIR=str(work/'target-control'))
subprocess.run(['cargo','build','--release','-p','nata','--manifest-path',str(base/'Cargo.toml')],env=env,check=True)
eager=work/'target-control/release/nata'
q=f"SELECT l_orderkey FROM '{file}'"
def args(binary,arm,limit=False):
    if arm=='scan':return [str(binary),'scan',str(file),'--columns','l_orderkey','--csv','--threads','8','--window','16']+(['--limit','0'] if limit else [])
    return [str(binary),'sql','-c',q+(' LIMIT 0' if limit else ''),'--csv','--threads','8','--window','16','--report']
rows=[]
for round in range(3):
    for name,binary,arm in [('fixed',fixed,'sql'),('scan',fixed,'scan'),('eager',eager,'sql')]:
        p=subprocess.run(args(binary,arm),stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,text=True,check=True)
        (out/f'{name}-{round}.txt').write_text(p.stderr)
        rss=int(re.search(r'peak_rss_kib=(\d+)',p.stderr)[1]);rows.append(dict(arm=name,round=round,rss_kib=rss))
base=subprocess.run(args(fixed,'sql',True),stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,text=True,check=True)
(out/'baseline.txt').write_text(base.stderr);baseline=int(re.search(r'peak_rss_kib=(\d+)',base.stderr)[1])
meta=pq.ParquetFile(file).metadata;largest=max(meta.row_group(i).num_rows for i in range(meta.num_row_groups))*8
bound=baseline+3*16*largest/1024+32768
result=dict(rows=meta.num_rows,row_groups=meta.num_row_groups,baseline_kib=baseline,decoded_group_bytes=largest,K=3,window=16,fixed_allowance_kib=32768,bound_kib=bound,runs=rows)
(out/'rss.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2),flush=True)
shim=work/'malloc-profile.so'
subprocess.run(['cc','-shared','-fPIC','-O2','bench/results/raw/20261107-debt-a/malloc-profile.c','-o',str(shim)],check=True)
for name,binary in [('fixed',fixed),('eager',eager)]:
    with (out/f'allocator-{name}.txt').open('w') as log:
        subprocess.run(args(binary,'sql'),stdout=subprocess.DEVNULL,stderr=log,env=dict(os.environ,LD_PRELOAD=str(shim.resolve())),check=True)
    profile=work/name
    with (out/f'heap-{name}-run.txt').open('w') as log:
        subprocess.run(['heaptrack','-o',str(profile),str(binary),'sql','-c','EXPLAIN ANALYZE '+q,'--threads','8','--window','16','--report'],stdout=log,stderr=subprocess.STDOUT,check=True)
    files=list(work.glob(name+'.*.zst'))+list(work.glob(name+'.zst'))+list(work.glob(name+'.gz'))
    assert len(files)==1,files
    with (out/f'heap-{name}.txt').open('w') as log:subprocess.run(['heaptrack_print','-f',str(files[0]),'-n','15'],stdout=log,check=True)
assert all(r['rss_kib']<=bound for r in rows if r['arm']=='fixed'),result
