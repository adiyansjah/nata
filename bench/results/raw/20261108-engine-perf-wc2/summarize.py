import json,statistics,pathlib,sys
part=sys.argv[1]; root=pathlib.Path('bench/results/raw/20261108-engine-perf-wc2'); p=root/f'{part}-screen'; d=json.loads((p/f'{part}-samples.json').read_text()); summ=json.loads((p/f'{part}-summary.json').read_text())
lines=['| Query | Threads | Before min / median s | After min / median s | Paired after/before | Control median [min,max] |','|---|---:|---:|---:|---:|---:|']
for c in summ['cells']:
 a=c['arms']; dist=lambda v: f"{v['min']:.6f} / {v['median']:.6f}"
 lines.append(f"| {c['query']} | {c['threads']} | {dist(a['before'])} | {dist(a['after'])} | {c['ratio']['median']:.6f} | {c['control_ratio']['median']:.6f} [{c['control_ratio']['min']:.6f},{c['control_ratio']['max']:.6f}] |")
# locate saved package-0 stats in samples
sample=d[0];print('sample keys',sample.keys()); print('summary keys',summ['cells'][0]['arms']['before'].keys())
(root/f'{part}-timing.md').write_text('\n'.join(lines)+'\n')
stages=[]
for q in ['q1','high_cardinality_tpch']:
 if not any(o['query']==q for o in d): continue
 for t in [1,8]:
  arms={a:[o for o in d if o['query']==q and o['threads']==t and o['arm']==a] for a in ['before','after','control']}
  for a,obs in arms.items():
   for o in obs:
    st=o.get('stats') or o.get('query_stats')
    if isinstance(st,str): st=json.loads(st)
    if not st:
     # bench_verdict.invoke uses these field names; fail visibly otherwise
     print(o.keys()); raise RuntimeError('missing stats')
    o['_stats']=st
  for name in arms['before'][0]['_stats']['stages']:
   v={a:statistics.median(o['_stats']['stages'][name]['service_wall_ns']/1e6 for o in obs) for a,obs in arms.items()}
   stages.append(dict(query=q,threads=t,stage=name,**v))
(root/f'{part}-stages.json').write_text(json.dumps(stages,indent=2)+'\n')
lines=['| Stage (service wall ms) | Q1 / 1t | Q1 / 8t | HC / 1t | HC / 8t |','|---|---:|---:|---:|---:|']
for name in dict.fromkeys(s['stage'] for s in stages):
 vals=[]
 for q,t in [('q1',1),('q1',8),('high_cardinality_tpch',1),('high_cardinality_tpch',8)]:
  s=next((s for s in stages if s['query']==q and s['threads']==t and s['stage']==name),None)
  vals.append(f"{s['before']:.3f} → {s['after']:.3f}" if s else 'missing')
 lines.append('| '+name+' | '+' | '.join(vals)+' |')
(root/f'{part}-stages.md').write_text('\n'.join(lines)+'\n')
