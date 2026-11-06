from driver import *
import signal
for n,mode in enumerate(['hand','pg']):
 port=15500+n;log=open(HOME/f'tmp-4b/verify-{mode}.log','w');p=subprocess.Popen([str(EXE),mode,str(port)],stderr=log)
 try:
  for _ in range(100):
   try:s,key=connect(port);break
   except ConnectionRefusedError:time.sleep(.05)
  s.setsockopt(socket.SOL_SOCKET,socket.SO_RCVBUF,1<<20)
  s.sendall(packet(b'Q',Q.encode()+b'\0'));nrows=0
  while True:
   t,b=msg(s)
   if t==b'D':
    assert struct.unpack('!h',b[:2])[0]==1
    v=b[6:].decode();assert v==str(nrows),(v,nrows);nrows+=1
   if t==b'Z':break
  assert nrows==1000000
  print(mode,'all million rows PASS',flush=True)
  # Raw unnamed extended no parameters.
  s.sendall(packet(b'P',b'\0'+(Q+' LIMIT 1').encode()+b'\0\0\0')+packet(b'B',b'\0\0\0\0\0\0\0\0')+packet(b'D',b'P\0')+packet(b'E',b'\0\0\0\0\0')+packet(b'S',b''))
  tags=[]
  while True:
   t,b=msg(s);tags.append(t.decode())
   if t==b'Z':break
  assert tags==['1','2','T','D','C','Z'],tags
  print(mode,'extended',tags,flush=True);s.close()
  # Real clients.
  env=dict(os.environ,LD_LIBRARY_PATH=str(HOME/'tmp-4b/clients/root/usr/lib/x86_64-linux-gnu'))
  psql=HOME/'tmp-4b/clients/root/usr/lib/postgresql/17/bin/psql'
  base=[str(psql),f'host=127.0.0.1 port={port} user=spike dbname=spike sslmode=disable','-X','-A','-t']
  r=subprocess.run(base+['-c',Q+' LIMIT 1'],capture_output=True,text=True,env=env,timeout=20);print(mode,'psql simple',r.returncode,repr(r.stdout),repr(r.stderr),flush=True)
  r=subprocess.run(base,input=Q+' LIMIT 1\n\\bind\n\\g\n',capture_output=True,text=True,env=env,timeout=20);print(mode,'psql bind',r.returncode,repr(r.stdout),repr(r.stderr),flush=True)
  out=open(HOME/f'tmp-4b/psql-cancel-{mode}.out','w');err=open(HOME/f'tmp-4b/psql-cancel-{mode}.err','w');ps=subprocess.Popen(base+['-c',Q],stdout=out,stderr=err,env=env);time.sleep(.2);ps.send_signal(signal.SIGINT);rc=ps.wait(timeout=20);out.close();err.close();print(mode,'psql cancel',rc,(HOME/f'tmp-4b/psql-cancel-{mode}.err').read_text(),flush=True)
  java=HOME/'tmp-4b/clients/root/usr/lib/jvm/java-21-openjdk-amd64/bin/java';cp=str(HOME/'tmp-4b')+':'+str(HOME/'tmp-4b/clients/root/usr/share/java/postgresql.jar')
  for qm in ['simple','extended']:
   r=subprocess.run([str(java),'-cp',cp,'Clients',str(port),qm],capture_output=True,text=True,timeout=30);print(mode,'jdbc',r.returncode,r.stdout,r.stderr,flush=True)
  r=subprocess.run([str(java),'-cp',cp,'Types',str(port)],capture_output=True,text=True,timeout=30);print(mode,'jdbc types',r.returncode,r.stdout,r.stderr,flush=True)
  r=subprocess.run([str(HOME/'tmp-4b/py/bin/python'),str(HOME/'tmp-4b/type-client.py'),str(port)],capture_output=True,text=True,timeout=30);print(mode,'psycopg types',r.returncode,r.stdout,r.stderr,flush=True)
 finally:p.terminate();p.wait();log.close()
