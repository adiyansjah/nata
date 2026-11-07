"""Synthetic loopback PostgreSQL codec and transparent, secret-redacting proxy."""
import socket, struct, threading, json
from pathlib import Path

def i(n): return struct.pack('!I', n)
def msg(t, b=b''): return t.encode()+i(len(b)+4)+b
def exact(s,n):
    b=b''
    while len(b)<n:
        x=s.recv(n-len(b))
        if not x: raise EOFError()
        b+=x
    return b
def frame(s):
    t=exact(s,1).decode(); n=struct.unpack('!I',exact(s,4))[0]
    assert 4<=n<=16777216
    return t,exact(s,n-4)
class Conn:
    def __init__(self,port=55432):
        self.s=socket.create_connection(('127.0.0.1',port),timeout=15)
        self.s.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1)
        b=i(196608)+b'user\0nata\0database\0nata\0application_name\0raw-reference\0\0'
        self.s.sendall(i(len(b)+4)+b); self.key=None
        while True:
            t,b=frame(self.s)
            if t=='K': self.key=b
            if t=='E': raise RuntimeError(b)
            if t=='Z': break
    def send(self,t,b=b''): self.s.sendall(msg(t,b))
    def until(self,tags=('Z',)):
        out=[]
        while True:
            t,b=frame(self.s); out.append([t,b.hex()])
            if t in tags: return out
    def query(self,q): self.send('Q',q.encode()+b'\0'); return self.until()
    def close(self): self.send('X'); self.s.close()
class Proxy:
    def __init__(self,path,port=55433):
        self.path=Path(path); self.lock=threading.Lock(); self.counter=0
        self.s=socket.socket(); self.s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
        self.s.bind(('127.0.0.1',port)); self.s.listen()
        threading.Thread(target=self.accept,daemon=True).start()
    def log(self,c,d,t,b):
        # Startup and BackendKeyData are omitted. No passwords or cancel keys.
        if t in ('K','p'): return
        with self.lock:
            with self.path.open('a') as f: f.write(json.dumps(dict(connection=c,direction=d,tag=t,hex=b.hex()))+'\n')
    def accept(self):
        while True:
            c,_=self.s.accept(); self.counter+=1
            threading.Thread(target=self.handle,args=(c,self.counter),daemon=True).start()
    def handle(self,c,num):
        p=None
        try:
            while True:
                n=struct.unpack('!I',exact(c,4))[0]; b=exact(c,n-4)
                code=struct.unpack('!I',b[:4])[0]
                if code in (80877103,80877104): c.sendall(b'N'); continue
                p=socket.create_connection(('127.0.0.1',55432))
                p.sendall(i(n)+b)
                if code==80877102: return
                break
            def relay(a,z,d):
                try:
                    while True:
                        t,b=frame(a); self.log(num,d,t,b); z.sendall(msg(t,b))
                except (EOFError,OSError):
                    try: z.shutdown(socket.SHUT_WR)
                    except OSError: pass
            th=threading.Thread(target=relay,args=(p,c,'backend')); th.start()
            relay(c,p,'frontend'); th.join()
        except (EOFError,OSError): pass
        finally:
            c.close()
            if p: p.close()

def sequences(out):
    out=Path(out); out.mkdir(exist_ok=True,parents=True)
    def save(name,steps): (out/(name+'.json')).write_text(json.dumps(dict(version=1,steps=steps),indent=2)+'\n')
    for rows in (0,1,2,3,4):
        c=Conn(); steps=[]
        def exchange(t,b,ends=('Z',)):
            c.send(t,b)
            front=[t,b.hex()]
            if ends!=('Z',): c.send('H'); front=[[t,b.hex()],['H','']]
            steps.append(dict(frontend=front,backend=c.until(ends)))
        exchange('Q',b'BEGIN\0')
        c.send('P',b's\0SELECT i::bigint FROM generate_series(1,'+str(rows).encode()+b') i\0\0\0')
        steps.append(dict(frontend=['P',(b's\0SELECT i::bigint FROM generate_series(1,'+str(rows).encode()+b') i\0\0\0').hex()]))
        c.send('B',b'p\0s\0'+b'\0\0'*3); steps.append(dict(frontend=['B',(b'p\0s\0'+b'\0\0'*3).hex()]))
        exchange('D',b'Pp\0',('T',))
        # Flush is required to observe suspension without ending the transaction.
        for _ in range(rows//2+2):
            c.send('E',b'p\0'+i(2)); c.send('H')
            steps.append(dict(frontend=[['E',(b'p\0'+i(2)).hex()],['H','']],backend=c.until(('s','C','E'))))
        exchange('S',b''); exchange('C',b'Pp\0',('3',)); exchange('S',b'')
        exchange('Q',b'ROLLBACK\0'); c.close(); save('execute-limit-'+str(rows),steps)
    # Parent amendment: cancel is inert while suspended; timeout is per Execute,
    # and excludes time awaiting the next Execute. Keys are never persisted.
    for name in ('suspended-cancel', 'suspended-timeout-restart'):
        c=Conn(); steps=[]
        def exchange(t,b,ends=('Z',)):
            c.send(t,b); front=[t,b.hex()]
            if ends != ('Z',): c.send('H'); front=[[t,b.hex()],['H','']]
            steps.append(dict(frontend=front,backend=c.until(ends)))
        exchange('Q', b'BEGIN\0')
        if name == 'suspended-timeout-restart':
            exchange('Q', b'SET statement_timeout=80\0')
            query=b'SELECT i::bigint, pg_sleep(0.03) FROM generate_series(1,3) i'
        else:
            query=b'SELECT i::bigint FROM generate_series(1,3) i'
        exchange('P', b's\0'+query+b'\0\0\0', ('1',))
        exchange('B', b'p\0s\0'+b'\0\0'*3, ('2',))
        exchange('D', b'Pp\0', ('T',))
        exchange('E', b'p\0'+i(1), ('s','C','E'))
        import time
        time.sleep(.12)
        if name == 'suspended-cancel':
            cancel=socket.create_connection(('127.0.0.1',55432)); cancel.sendall(i(16)+i(80877102)+c.key)
            # Read EOF to establish that the server processed cancellation before resume.
            assert cancel.recv(1)==b''; cancel.close()
            steps.append(dict(frontend=['H',''], concurrent='CancelRequest with session key (redacted)', sleep_ms=120))
        else:
            steps.append(dict(frontend=['H',''], sleep_ms=120))
        for _ in range(3):
            exchange('E', b'p\0'+i(1), ('s','C','E'))
            # After an ErrorResponse PostgreSQL ignores everything until Sync, so stop resuming and record
            # the error as the outcome (found 2026-11-07: the first version waited forever here).
            if steps[-1]['backend'] and steps[-1]['backend'][-1][0] == 'E': break
        exchange('S',b''); exchange('Q',b'ROLLBACK\0')
        c.close(); save(name,steps)
    c=Conn(); steps=[]
    q='SET statement_timeout=20'
    steps.append(dict(frontend=['Q',(q+'\0').encode().hex()],backend=c.query(q)))
    for t,b in [('P',b's\0SELECT pg_sleep(0.2)\0\0\0'),('B',b'p\0s\0'+b'\0\0'*3),('E',b'p\0'+i(0)),('H',b'')]:
        c.send(t,b);steps.append(dict(frontend=[t,b.hex()]))
    steps[-1]['backend']=c.until(('E',))
    c.send('P',b'ignored\0SELECT 9\0\0\0');steps.append(dict(frontend=['P',b'ignored\0SELECT 9\0\0\0'.hex()]))
    c.send('S');steps.append(dict(frontend=['S',''],backend=c.until()))
    steps.append(dict(frontend=['Q',b'SELECT 1\0'.hex()],backend=c.query('SELECT 1')))
    c.close();save('extended-timeout-sync',steps)
    cases={
      'sql-cursor':['BEGIN','DECLARE c NO SCROLL CURSOR FOR SELECT i::bigint FROM generate_series(1,4) i','FETCH 2 FROM c','FETCH 2 FROM c','FETCH 2 FROM c','CLOSE c','COMMIT'],
      'timeout':['SET statement_timeout=20', 'SELECT pg_sleep(0.2)','SELECT 1'],
      'rollback':['BEGIN','SELECT 1/0','SELECT 1','ROLLBACK','SELECT 1'],
      'copy':["COPY (SELECT * FROM public.all_types) TO STDOUT (FORMAT binary)","COPY (SELECT * FROM public.all_types) TO STDOUT", "COPY public.all_types TO STDOUT WITH (FORMAT csv, HEADER true)"]}
    for name,queries in cases.items():
        c=Conn(); steps=[]
        for q in queries: steps.append(dict(frontend=['Q',(q+'\0').encode().hex()],backend=c.query(q)))
        c.close(); save(name,steps)
    c=Conn(); c.send('Q',b'SELECT pg_sleep(10)\0')
    import time
    time.sleep(.05)
    cancel=socket.create_connection(('127.0.0.1',55432)); cancel.sendall(i(16)+i(80877102)+c.key); cancel.close()
    save('concurrent-cancel',[dict(frontend=['Q',b'SELECT pg_sleep(10)\0'.hex()],concurrent='CancelRequest with session key (redacted)',backend=c.until()),dict(frontend=['Q',b'SELECT 1\0'.hex()],backend=c.query('SELECT 1'))]); c.close()
