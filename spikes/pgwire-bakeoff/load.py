from driver import *
for k,mode in enumerate(['hand','pg']):
 port=15600+k;log=open(HOME/f'tmp-4b/load-{mode}.log','w');p=subprocess.Popen([str(EXE),mode,str(port)],stderr=log)
 try:
  for _ in range(100):
   try:s,key=connect(port);s.close();break
   except ConnectionRefusedError:time.sleep(.05)
  clients=[]
  for _ in range(8):
   s,key=connect(port);s.sendall(packet(b'Q',Q.encode()+b'\0'));clients.append(s)
  samples=[]
  for _ in range(20):samples.append(metrics(p.pid));time.sleep(.05)
  print(mode,'eight pending queries',max(samples,key=lambda d:d['Threads']),flush=True)
  for s in clients:s.close()
  time.sleep(.5);print(mode,'after disconnect',metrics(p.pid),flush=True)
 finally:p.terminate();p.wait();log.close()
