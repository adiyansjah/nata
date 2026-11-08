import importlib.util, pathlib, subprocess, os, csv, collections, json, sys
sys.path.insert(0,str(pathlib.Path('scripts').resolve()))
spec=importlib.util.spec_from_file_location('h','scripts/bench-aggregate-perf1-heavy.py');h=importlib.util.module_from_spec(spec);spec.loader.exec_module(h)
a,b,part=sys.argv[1:];out=[];tmp=pathlib.Path('/tmp/nata-wc2')
for query in h.QUERIES:
 sql=pathlib.Path(f'check/queries/aggregate/{query}.sql').read_text().replace('{data}','/home/dev/nata-s5-fixes/sf1')
 for t in [1,8]:
  rows=[]
  for binary in [a,b]:
   r=subprocess.run(h.command(pathlib.Path(binary),sql,t,8<<30,tmp),capture_output=True,env={k:v for k,v in os.environ.items() if k!='AI_AGENT'},check=True)
   rows.append(collections.Counter(tuple(r) for r in list(csv.reader(r.stdout.decode().splitlines()))[1:]))
  assert rows[0]==rows[1],(query,t,'RESULT CHANGED')
  out.append(dict(query=query,threads=t,exact=True,rows=sum(rows[0].values())))
pathlib.Path(f'/tmp/nata-wc2/{part}-exact.json').write_text(json.dumps(out,indent=2))
