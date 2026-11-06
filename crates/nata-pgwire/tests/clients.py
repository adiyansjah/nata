import psycopg,sys,datetime,decimal,threading,time
port=sys.argv[1]; f=sys.argv[2]; large=sys.argv[3]
print('psycopg',psycopg.__version__,'libpq',psycopg.pq.version())
def conn(**kw):return psycopg.connect(host='127.0.0.1',port=port,user='test',dbname='nata',**kw)
q=f"SELECT * FROM '{f}'"
with conn() as c:
 r=c.execute(q).fetchall()
 assert [type(v) for v in r[0]]==[bool,str,int,int,int,float,float,decimal.Decimal,str,datetime.date]
 assert r[0][:5]==(True,'-128',-32768,-2147483648,-9223372036854775808)
 assert str(r[0][6])=='nan' and str(r[0][5])=='-0.0' and r[0][7:]==(decimal.Decimal('123.450'),'',datetime.date(1970,1,1))
 assert r[1]==(False,'127',32767,2147483647,9223372036854775807,float('inf'),-float('inf'),decimal.Decimal('-0.001'),"secret-cell-a,b\né",datetime.date(2024,2,29))
 assert all(v is None for v in r[2])
 print('DEFAULT connect/simple/type rows',[(type(v).__name__,str(v)) for v in r[0]])
 for i in range(7):
  try:c.execute(q).fetchall();print('DEFAULT repeat',i+2,'PASS')
  except psycopg.Error as e:
   assert i==4 and e.sqlstate=='0A000'
   print('DEFAULT repeat',i+2,e.sqlstate,str(e));break
 else:raise AssertionError('expected named-prepare refusal on execution six')
with conn() as c:
 try:c.execute(f"SELECT %s FROM '{f}' LIMIT 1",(7,)).fetchone()
 except psycopg.Error as e:
  assert e.sqlstate=='0A000'
  print('DEFAULT integer parameter',e.sqlstate,str(e))
 else:raise AssertionError('expected binary parameter refusal')
with conn(prepare_threshold=None) as c:
 for i in range(7):assert c.execute(f"SELECT %t FROM '{f}' LIMIT 1",(7,)).fetchone()==(7,)
 assert c.execute(f"SELECT CAST(%t AS INTEGER) FROM '{f}' LIMIT 1",(None,)).fetchone()==(None,)
 print('TEXT configured parameter/repeat 7/NULL PASS')
 try:c.execute(f"SELECT nope FROM '{f}'")
 except psycopg.Error as e:print('error',e.sqlstate,e.diag.statement_position,str(e));assert e.sqlstate=='42703' and e.diag.statement_position=='8'
 c.rollback()
with conn(prepare_threshold=None) as c:
 timer=threading.Timer(.15,c.cancel);timer.start()
 try:c.execute(f"SELECT s FROM '{large}'").fetchall();raise AssertionError('cancel did not interrupt')
 except psycopg.Error as e:print('cancel',e.sqlstate);assert e.sqlstate=='57014'
 finally:timer.join()
print('PASS')
