import socket,struct,time,subprocess,pathlib,json,threading,sys,os
ROOT=pathlib.Path('/home/dev/src/nata');HOME=pathlib.Path.home();EXE=ROOT/'spikes/pgwire-bakeoff/target/debug/pgwire-bakeoff'
Q="SELECT v FROM '/home/dev/tmp-4b/million.parquet'"
def packet(t,b):return t+struct.pack('!i',len(b)+4)+b
def exact(s,n):
 b=b''
 while len(b)<n:
  x=s.recv(n-len(b))
  if not x:raise EOFError()
  b+=x
 return b
def msg(s):
 t=exact(s,1);n=struct.unpack('!i',exact(s,4))[0];return t,exact(s,n-4)
def connect(port):
 s=socket.create_connection(('127.0.0.1',port));s.settimeout(10);s.setsockopt(socket.SOL_SOCKET,socket.SO_RCVBUF,4096)
 s.sendall(struct.pack('!i',len(b'user\0spike\0\0')+8)+struct.pack('!i',196608)+b'user\0spike\0\0')
 key=None
 while True:
  t,b=msg(s)
  if t==b'K':key=b
  if t==b'Z':return s,key

def metrics(pid):
 d={}
 for l in pathlib.Path(f'/proc/{pid}/status').read_text().splitlines():
  if l.startswith(('VmRSS:','VmHWM:','Threads:')):k,v,*_=l.split();d[k[:-1]]=int(v)
 d['names']=sorted(p.read_text().strip() for p in pathlib.Path(f'/proc/{pid}/task').glob('*/comm'))
 return d

def main():
 records=[]
 for run,(mode,bad) in enumerate([('hand',False),('pg',False),('hand',False),('pg',False),('hand',True),('pg',True)]):
  port=15432+run;log=open(HOME/f'tmp-4b/server-{run}.log','w');p=subprocess.Popen([str(EXE),mode,str(port)]+(['unbounded']if bad else []),stderr=log)
  try:
   for _ in range(100):
    try:s,key=connect(port);break
    except ConnectionRefusedError:time.sleep(.05)
   baseline=metrics(p.pid);s.sendall(packet(b'Q',(Q+" LIMIT 1").encode()+b'\0'))
   got=[]
   while True:
    t,b=msg(s)
    if t==b'D':got.append(b)
    if t==b'Z':break
   assert got==[b'\x00\x01\x00\x00\x00\x010'],got
   s.sendall(packet(b'Q',Q.encode()+b'\0'))
   # Intentionally consume only a few KiB per 50 ms for 4 seconds, then stop reading.
   samples=[];read=0
   for _ in range(80):
    time.sleep(.05);samples.append(metrics(p.pid))
    s.settimeout(.01)
    try:read+=len(s.recv(4096))
    except socket.timeout:pass
   s.settimeout(10)
   # Query remains pending; cancel via separate connection, then drain.
   c=socket.create_connection(('127.0.0.1',port));c.sendall(struct.pack('!ii',16,80877102)+key);c.close()
   s.close();time.sleep(.3)
   record={'mode':mode,'unbounded':bad,'baseline':baseline,'peak_rss':max(x['VmRSS']for x in samples),'peak_hwm':max(x['VmHWM']for x in samples),'peak_threads':max(x['Threads']for x in samples),'sample_names':max(samples,key=lambda x:x['Threads'])['names'],'rss_first':samples[0]['VmRSS'],'rss_last':samples[-1]['VmRSS'],'read_bytes':read}
   log.flush();import re
   progress=re.findall(r'progress produced=(\d+)',(HOME/f'tmp-4b/server-{run}.log').read_text());record['progress']=[int(n) for n in progress]
   records.append(record);print(json.dumps(record),flush=True)
  finally:p.terminate();p.wait();log.close()
 (HOME/'tmp-4b/memory.json').write_text(json.dumps(records,indent=2))
if __name__=='__main__':main()
