"""Diagnostic baseline, not the skeleton gate. Cell FAILs do not change exit status.
Run from the repo root after the setup in the result record.
"""
import os,sys,json,socket,subprocess,time,threading,struct
from pathlib import Path
import psycopg
work=Path.home()/'tmp-wcskel'
root=Path.cwd()
fixture=work/'data.parquet'
reservation=socket.socket();reservation.bind(('127.0.0.1',0));port=reservation.getsockname()[1];reservation.close()
dsn=f'host=127.0.0.1 port={port} user=nata dbname=nata'
query=f"SELECT v FROM '{fixture}' LIMIT 3"
results={}
expected=b'PGCOPY\n\xff\r\n\x00'+bytes(8)+b''.join(struct.pack('!hiq',1,8,n) for n in range(3))+struct.pack('!h',-1)
def record(name,f):
 try:
  f();results[name]='PASS'
 except Exception as e:
  results[name]='FAIL '+str(getattr(e,'sqlstate',None) or getattr(e,'returncode',None) or type(e).__name__)
 print(name,results[name],flush=True)
def cli(q):
 return subprocess.run(['psql',dsn,'-c',q],capture_output=True,timeout=15,check=True)
log=(work/'logs/server.txt').open('w')
p=subprocess.Popen([str(work/'target/debug/nata'),'serve','--listen',f'127.0.0.1:{port}','--threads','2','--max-queries','2'],stdout=log,stderr=log)
try:
 for _ in range(100):
  try:
   with socket.create_connection(('127.0.0.1',port),timeout=.1): pass
   break
  except OSError:
   if p.poll() is not None: raise RuntimeError('server exited')
   time.sleep(.02)
 for name,q in [('connect','SELECT 1'),('select',query),('d','\\d public.all_types'),('dt','\\dt'),('dn','\\dn'),('df','\\df'),('l','\\l'),('timeout','SET statement_timeout=20')]:
  record('psql '+name,lambda q=q:cli(q))
 def cp():
  b=cli(f'COPY ({query}) TO STDOUT (FORMAT binary)').stdout
  assert b==expected
 record('psql copy',cp)
 with psycopg.connect(dsn) as c:
  record('psycopg connect',lambda:None)
  def sel():
   for _ in range(10):assert c.execute(query).fetchall()==[(0,),(1,),(2,)]
  record('psycopg select',sel)
  for name,q in [('tables','SELECT * FROM information_schema.tables WHERE table_schema=%s'),('columns','SELECT * FROM information_schema.columns WHERE table_schema=%s')]:
   record('psycopg '+name,lambda q=q:c.execute(q,('public',)).fetchall());c.rollback()
  record('psycopg types',lambda:c.execute('SELECT n.nspname,t.typname,t.oid,t.typtypmod FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace').fetchall());c.rollback()
  record('psycopg timeout',lambda:c.execute('SET statement_timeout=20'));c.rollback()
  def cp2():
   with c.cursor().copy(f'COPY ({query}) TO STDOUT (FORMAT binary)') as copy: b=b''.join(bytes(x) for x in copy)
   assert b==expected
  record('psycopg copy',cp2)
  def recover():
   try:c.execute(f"SELECT v/0 FROM '{fixture}' LIMIT 1").fetchall();raise AssertionError('no error')
   except psycopg.errors.DivisionByZero:pass
   c.rollback();sel()
  record('psycopg error-recovery',recover)
  def cursor():
   try:
    with c.cursor(name='skeleton') as cur:cur.execute(query);cur.fetchmany(2)
   except psycopg.errors.FeatureNotSupported:return
   raise AssertionError('cursor accepted')
  record('psycopg SQL-cursor-refusal',cursor);c.rollback()
  def cancel():
   def request():time.sleep(.05);c.cancel()
   t=threading.Thread(target=request);t.start()
   try:
    c.execute(f"SELECT v FROM '{fixture}'").fetchall();raise AssertionError('not canceled')
   except psycopg.errors.QueryCanceled:pass
   finally:t.join()
   c.rollback();sel()
  record('psycopg cancel-recovery',cancel)
  def malformed():
   sys.path.insert(0,str(root/'scripts/wirecapture'))
   from protocol import Conn,i,frame
   raw=Conn(port);raw.s.sendall(b'Q'+i(3))
   tag,body=frame(raw.s);assert tag=='E' and b'C08P01\0' in body
   raw.s.close();sel()
  record('raw malformed-frame-isolation',malformed)
 jdk=Path.home()/'tmp-4b/clients/root/usr/lib/jvm/java-21-openjdk-amd64/bin'
 jar=Path.home()/'tmp-4b/clients/root/usr/share/java/postgresql-42.7.7.jar'
 subprocess.run([str(jdk/'javac'),'-cp',str(jar),str(work/'Baseline.java')],check=True,capture_output=True,timeout=30)
 j=subprocess.run([str(jdk/'java'),'-cp',str(work)+os.pathsep+str(jar),'Baseline',str(port),str(fixture)],capture_output=True,text=True,timeout=60)
 print(j.stdout,flush=True);assert j.returncode==0,(j.returncode,j.stderr)
 (work/'logs/jdbc.txt').write_text(j.stdout)
finally:
 p.terminate()
 try:p.wait(timeout=10)
 except subprocess.TimeoutExpired:p.kill();p.wait()
 log.close()
 (work/'logs/clients.json').write_text(json.dumps(results,indent=2)+'\n')
