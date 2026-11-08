import argparse, importlib.util, os, pathlib
repo=pathlib.Path(__file__).resolve().parents[4]
p=argparse.ArgumentParser(description='Independent RSS/reservation controls; unsettled, not paired timing')
for name in ['before','after','data','out']: p.add_argument('--'+name,type=pathlib.Path,required=True)
a=p.parse_args()
a.before=a.before.resolve();a.after=a.after.resolve();a.data=a.data.resolve()
for binary in [a.before,a.after]:
 if binary.stat().st_mode & 0o222: p.error('use immutable binary copies')
spec=importlib.util.spec_from_file_location('c',repo/'scripts/bench-aggregate-closeout.py')
c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
env=dict(os.environ,TMPDIR=str(out));env.pop('AI_AGENT',None)
data_sql=str(a.data).replace("'", "''")
query=(repo/'check/queries/aggregate/high_cardinality_tpch.sql').read_text().replace('{data}',data_sql)
c.h.save(out/'identity.json',dict(input_sha256=c.h.digest(a.data/'lineitem.parquet'),binaries={k:c.h.digest(v) for k,v in [('before',a.before),('after',a.after)]}))
records=[]
for threads in [1,8]:
 for budget in [4294967296,67108864]:
  for label,binary in [('before',a.before),('after',a.after)]:
   stem=f'{threads}-{budget}-{label}'
   def run(sql,suffix):
    return c.invoke(c.h.command(binary,sql,threads,budget,out),env,out/'output.csv',out/(stem+suffix+'.log'),600)
   baseline=run(f"SELECT l_orderkey FROM '{data_sql}/lineitem.parquet' LIMIT 0",'-baseline')
   assert baseline['exit']==0, baseline
   row=run(query,'')
   c.resource_gate(row,budget,baseline['rss_bytes'],4<<30)
   records.append(dict(threads=threads,budget=budget,arm=label,baseline_rss=baseline['rss_bytes'],**row))
   (out/'output.csv').unlink()
   c.h.save(out/'observations.json',records)
