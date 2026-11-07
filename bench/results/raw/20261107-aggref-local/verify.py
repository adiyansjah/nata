from pathlib import Path
import csv
import itertools
root=Path(__file__).resolve().parents[4]

def unrestricted_complete(rows):
 for sf,t,q,arm in itertools.product(['1','10'],['1','4','8'],['q1','having','agg_orderkey','agg_partkey'],['control','twin']):
  group=[r for r in rows if (r['sf'],r['threads'],r['query'],r['arm'],r['memory'])==(sf,t,q,arm,'4GiB')]
  if len(group)!=5 or {r['round'] for r in group}!={'0','1','2','3','4'} or any(r['ok']!='True' or r['settled']!='True' for r in group):return False
 return True
for cpu in ['arm64','x86']:
 base=root/'bench/results/raw'/('20261107-aggref-'+cpu)/'results/aggref'
 rows=list(csv.DictReader((base/'runs.csv').open()))
 assert unrestricted_complete(rows)
 for sf,mem in [('1','64MiB'),('10','256MiB')]:
  fixture=[r for r in rows if r['sf']==sf and r['memory']==mem and r['query']=='having']
  assert len(fixture)==30 and all(r['ok']=='True' and int(r['temp_peak_bytes'])>0 for r in fixture)
 first=next(i for i,r in enumerate(rows) if r['memory']=='4GiB')
 bad=rows[:first]+rows[first+1:]
 assert not unrestricted_complete(bad)
 med=list(csv.DictReader((base/'medians.csv').open()))
 for r in med:
  if r['n']!='5':continue
  group=[x for x in rows if all(x[k]==r[k] for k in ['sf','threads','query','memory','arm']) and x['ok']=='True']
  values=sorted(float(x['seconds']) for x in group)
  assert len(values)==5
  for field,v in zip(['min_s','q1_s','median_s','q3_s','max_s'],values):assert float(r[field])==v
  assert all(int(x['rss_kib'])>0 and int(x['output_bytes'])>0 for x in group)
 print(cpu, 'PASS: unrestricted 48 arm/cells complete; planted missing round rejected; all full-cell medians/quartiles match raw order statistics')

scale=root/'bench/results/raw/20261107-aggref-sf100-arm64-r2/results/aggref'
if (scale/'completion.json').exists():
 rows=list(csv.DictReader((scale/'runs.csv').open()))
 expected=[(str(t),'q1','32GiB') for t in [8,16,32]]+[('8','having',m) for m in ['32GiB','4GiB']]
 assert len(rows)==50
 for t,q,m in expected:
  for arm in ['control','twin']:
   group=[r for r in rows if (r['threads'],r['query'],r['memory'],r['arm'])==(t,q,m,arm)]
   assert len(group)==5 and {r['round'] for r in group}=={'0','1','2','3','4'}
   assert all(r['ok']=='True' and r['settled']=='True' for r in group)
   if m=='4GiB':assert all(int(r['temp_peak_bytes'])>0 for r in group)
 for m in csv.DictReader((scale/'medians.csv').open()):
  vals=sorted(float(r['seconds']) for r in rows if all(r[k]==m[k] for k in ['sf','threads','query','memory','arm']))
  for field,v in zip(['min_s','q1_s','median_s','q3_s','max_s'],vals):assert float(m[field])==v
 print('SF100 PASS: all five cells complete and settled; every 4 GiB HAVING round spills; summaries match raw')
