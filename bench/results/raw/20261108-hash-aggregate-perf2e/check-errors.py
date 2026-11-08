import argparse,json,os,subprocess
from pathlib import Path
p=argparse.ArgumentParser(); p.add_argument('--before',required=True); p.add_argument('--after',required=True); p.add_argument('--data',required=True); p.add_argument('--out',type=Path,required=True); a=p.parse_args()
queries=[
"SELECT id, 1 // (id - 2) AS v FROM '{data}/sqlcheck/types.parquet' GROUP BY id HAVING id = 2",
"SELECT id, 1 // (id - 2) AS v FROM '{data}/sqlcheck/types.parquet' GROUP BY id HAVING id <> 2",
"SELECT id, 1 // (id - 2) AS v FROM '{data}/sqlcheck/types.parquet' GROUP BY id HAVING id = 2 LIMIT 0",
"SELECT sum(1 // (id - 2)) FROM '{data}/sqlcheck/types.parquet' HAVING false LIMIT 0",
"SELECT id, 1 // (id - 2) AS v, CAST('bad' AS INTEGER) AS w FROM '{data}/sqlcheck/types.parquet' GROUP BY id HAVING id = 2",
]
env=dict(os.environ);env.pop('AI_AGENT',None);env.pop('CODEX_CI',None)
results=[]
for template in queries:
    sql=template.replace('{data}',a.data)
    arms=[]
    for binary in [a.before,a.after]:
        r=subprocess.run([binary,'sql','--csv','--threads','1','--command='+sql],env=env,capture_output=True,text=True,check=False)
        arms.append(dict(exit=r.returncode,stdout=r.stdout,stderr=r.stderr))
    assert arms[0]==arms[1],(template,arms)
    results.append(dict(query=template,**arms[0]))
a.out.write_text(json.dumps(results,indent=2)+'\n')
print('5 exact stdout/stderr/exit comparisons pass')
