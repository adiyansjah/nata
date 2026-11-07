import json,statistics,collections,sys
t=json.load(open(sys.argv[1]))
c=collections.defaultdict(list)
for r in t:
    if r.get('exit',0)==0 and r.get('role','primary') in ('primary',None,'') : c[(r['query'],r['threads'],r['budget']>>20,r['variant'])].append(r['seconds'])
roles=collections.Counter(r.get('role') for r in t)
cells=sorted({k[:3] for k in c})
print('roles',dict(roles))
for cell in cells:
    m={v:statistics.median(c[cell+(v,)]) for v in ('baseline','morsel','smallgroup','fusion','duckdb') if c.get(cell+(v,))}
    b=m.get('baseline');d=m.get('duckdb')
    print(f"{cell[0][:4]:5}{cell[1]:>2}t {('mem' if cell[2]>1000 else '64M'):4}", ' '.join(f"{v}={m[v]:.3f}" for v in m), f"| fusion/base={m['fusion']/b:.2f} fusion/duck={m['fusion']/d:.1f}x" if d else '')
