import psycopg,sys
print('psycopg',psycopg.__version__)
with psycopg.connect(f'host=127.0.0.1 port={sys.argv[1]} dbname=spike user=spike sslmode=disable',autocommit=True) as c:
 with c.cursor() as cur:
  cur.execute("SELECT * FROM '/home/dev/tmp-4b/types.parquet'",prepare=False)
  print('oids',[d.type_code for d in cur.description]);r=cur.fetchone();print([(type(v).__name__,str(v)) for v in r])
