"""Reduce captured Parse/Query SQL into an auditable conservative inventory.
Requires pglast. Unresolved references stay explicit; they never count as covered.
"""
import json,sys,re
from pathlib import Path
from collections import defaultdict
from pglast import parser
raw=Path(sys.argv[1]); adr=Path(sys.argv[2]).read_text(); dest=Path(sys.argv[3])
labels=[json.loads(x) for x in (raw/'action-connections.jsonl').read_text().splitlines()]
catalog=json.loads((raw/'catalog-columns.json').read_text())
known={};helpers=set('version current_database current_schema current_schemas current_setting format_type pg_table_is_visible pg_type_is_visible pg_function_is_visible pg_get_userbyid pg_get_expr pg_get_constraintdef pg_get_indexdef pg_get_viewdef obj_description col_description pg_encoding_to_char'.split())
for line in adr.splitlines():
 if not line.startswith('| '):continue
 cells=line.split('|'); rels=cells[1].strip(); cols=cells[2]
 names=re.findall(r'(?:information_schema|pg_catalog)\.\w+',rels)
 if not names:continue
 if len(names)==1:known[names[0]]=set(re.findall(r'\b[a-z][a-z0-9_]*\b',cols))
# Explicit grouped rows in ADR 0014.
for rel,cols in {
 'pg_index':'indexrelid indrelid indisunique indisprimary indkey','pg_indexes':'schemaname tablename indexname tablespace indexdef','pg_constraint':'oid conname connamespace contype conrelid confrelid conkey confkey','pg_attrdef':'oid adrelid adnum adbin','pg_description':'objoid classoid objsubid description','pg_inherits':'inhrelid inhparent inhseqno','pg_roles':'oid rolname','pg_user':'usesysid usename','pg_tablespace':'oid spcname spcowner','pg_am':'oid amname amtype','pg_collation':'oid collname collnamespace collowner collprovider collisdeterministic collencoding collcollate collctype','pg_tables':'schemaname tablename tableowner tablespace hasindexes hasrules hastriggers rowsecurity','pg_views':'schemaname viewname viewowner definition'}.items():known['pg_catalog.'+rel]=set(cols.split())
def walk(x):
 if isinstance(x,dict):
  yield x
  for v in x.values():yield from walk(v)
 elif isinstance(x,list):
  for v in x:yield from walk(v)
def strings(x):return [v['String'].get('sval',v['String'].get('str')) for v in x if 'String'in v]
items=defaultdict(set);sqls=[];unresolved=[]
for line in (raw/'protocol.jsonl').open():
 e=json.loads(line)
 if e['direction']!='frontend' or e['tag'] not in ('Q','P'):continue
 b=bytes.fromhex(e['hex']);sql=(b.split(b'\0',2)[1] if e['tag']=='P' else b.split(b'\0')[0]).decode()
 client=next((x['action'].split()[0] for x in labels if x['first']<=e['connection']<=x['last']),'unlabelled')
 if (client,sql) in sqls:continue
 sqls.append((client,sql))
 try: tree=json.loads(parser.parse_sql_json(sql))
 except Exception as err: unresolved.append(dict(client=client,sql=sql,error=str(err)));continue
 def visit(x,aliases=None,rels=None):
  aliases={} if aliases is None else aliases
  rels=set() if rels is None else rels
  if isinstance(x,list):
   for v in x:visit(v,aliases,rels)
   return
  if not isinstance(x,dict):return
  if 'SelectStmt' in x:
   stmt=x['SelectStmt'];local=dict(aliases);direct=set()
   def from_item(v):
    if isinstance(v,list):
     for child in v:from_item(child)
    elif isinstance(v,dict):
     if 'RangeVar'in v:
      r=v['RangeVar'];name=r.get('schemaname','pg_catalog')+'.'+r['relname']
      if name in catalog:
       direct.add(name);local[r['relname']]=name;items[(client,name)]
       if 'alias'in r:local[r['alias'].get('Alias',r['alias'])['aliasname']]=name
     elif 'RangeSubselect'in v:
      r=v['RangeSubselect']
      if 'alias'in r:local[r['alias'].get('Alias',r['alias'])['aliasname']]=None
     else:
      for child in v.values():from_item(child)
   from_item(stmt.get('fromClause',[]))
   for key,v in stmt.items():
    visit({"SelectStmt":v} if key in ("larg","rarg") else v,local,direct)
   return
  if 'ColumnRef'in x:
   fields=x['ColumnRef']['fields'];f=strings(fields);star=any('A_Star'in v for v in fields)
   if len(f)>=2 and f[-2]in aliases:candidates={aliases[f[-2]]}-{None}
   elif star and f and f[-1]in aliases:candidates={aliases[f[-1]]}-{None}
   elif len(f)>=3 and '.'.join(f[-3:-1])in rels:candidates={'.'.join(f[-3:-1])}
   elif len(f)==1 and not star:candidates={r for r in rels if f[0]in catalog[r]}
   elif star:candidates=rels
   else:candidates=set()
   for r in candidates:items[(client,r)].update(catalog[r] if star else [f[-1]])
  if 'SQLValueFunction'in x:
   name=x['SQLValueFunction']['op'].removeprefix('SVFOP_').lower();items[(client,'function:'+name)]
  if 'FuncCall'in x:
   name='.'.join(strings(x['FuncCall']['funcname']));items[(client,'function:'+name)]
  for v in x.values():visit(v,aliases,rels)
 visit(tree)
rows=[]
for (client,name),cols in sorted(items.items()):
 if name.startswith('function:'):
  missing=[]; covered=name.split('.')[-1].removeprefix('function:')in helpers
 else:
  missing=sorted(cols-known.get(name,set()));covered=name in known and not missing
 rows.append(dict(client=client,object=name,columns=sorted(cols),covered=covered,missing_columns=missing))
dest.mkdir(parents=True,exist_ok=True)
(dest/'inventory.json').write_text(json.dumps(rows,indent=2)+'\n')
(dest/'sql.json').write_text(json.dumps([dict(client=c,sql=s)for c,s in sqls],indent=2)+'\n')
(dest/'unresolved.json').write_text(json.dumps(unresolved,indent=2)+'\n')
lines=['| Client | Relation or function | Columns read | Covered / missing |','|---|---|---|---|']
for r in rows:lines.append('| '+r['client']+' | '+r['object']+' | '+', '.join(r['columns'])+' | '+('covered' if r['covered'] else 'missing: '+(', '.join(r['missing_columns'])or 'object/helper'))+' |')
(dest/'inventory.md').write_text('\n'.join(lines)+'\n')
print('covered',sum(r['covered'] for r in rows),'missing',sum(not r['covered']for r in rows),'parse failures',len(unresolved))
