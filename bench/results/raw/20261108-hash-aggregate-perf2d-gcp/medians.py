import json,statistics,collections,sys
t=json.load(open(sys.argv[1]));order=sys.argv[2].split(',')
c=collections.defaultdict(list)
for r in t:
    if r.get('exit',0)==0 and r.get('role')=='primary': c[(r['query'],r['threads'],r['budget']>>20,r['variant'])].append(r['seconds'])
for cell in sorted({k[:3] for k in c}):
    m={v:statistics.median(c[cell+(v,)]) for v in order+['duckdb'] if c.get(cell+(v,))}
    b=m[order[0]];f=m[order[-1]];d=m.get('duckdb')
    print(f"{cell[0][:4]:5}{cell[1]:>2}t {('mem' if cell[2]>1000 else '64M'):4}", ' '.join(f"{v}={m[v]:.3f}" for v in m), f"| final/base={f/b:.2f} final/duck={f/d:.1f}x" if d else '')
