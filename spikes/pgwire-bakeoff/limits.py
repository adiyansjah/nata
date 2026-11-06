from driver import *
for j,mode in enumerate(['hand','pg']):
 port=15700+j;log=open(HOME/f'tmp-4b/limits-{mode}.log','w');p=subprocess.Popen([str(EXE),mode,str(port)],stderr=log)
 try:
  for _ in range(100):
   try:s,key=connect(port);break
   except ConnectionRefusedError:time.sleep(.05)
  s.close()
  for n in [3,-1,1114113]:
   s,key=connect(port);before=metrics(p.pid);s.sendall(b'Q'+struct.pack('!i',n));s.settimeout(2);assert s.recv(1)==b'';s.close();print(mode,'refused length',n,'RSS before/after',before['VmRSS'],metrics(p.pid)['VmRSS'])
  a,key=connect(port);b,kb=connect(port);a.sendall(packet(b'Q',Q.encode()+b'\0'));time.sleep(.1)
  c=socket.create_connection(('127.0.0.1',port));c.sendall(struct.pack('!ii',16,80877102)+key);c.close()
  codes=[]
  while True:
   t,body=msg(a)
   if t==b'E':codes.append(body)
   if t==b'Z':break
  assert any(b'57014' in x for x in codes),codes
  for s in [a,b]:
   s.sendall(packet(b'Q',(Q+' LIMIT 1').encode()+b'\0'));rows=0
   while True:
    t,body=msg(s)
    if t==b'D':rows+=1
    if t==b'Z':break
   assert rows==1;s.close()
  print(mode,'cancel error 57014; same and other session subsequent query PASS',flush=True)
 finally:p.terminate();p.wait();log.close()
