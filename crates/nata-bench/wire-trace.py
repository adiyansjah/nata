#!/usr/bin/env python3
"""Transparent loopback frontend trace for client probes; never log keys or Bind data."""
import socket
import struct
import sys
import threading

listen, target, path = sys.argv[1:]
lock=threading.Lock()
def log(value):
    with lock, open(path,'a') as f:f.write(value+'\n')
def session(front):
    back=socket.create_connection(('127.0.0.1',int(target)),timeout=30)
    def replies():
        try:
            while b:=back.recv(65536):front.sendall(b)
        except OSError:pass
        finally:
            try:front.shutdown(socket.SHUT_WR)
            except OSError:pass
    threading.Thread(target=replies,daemon=True).start()
    pending=b'';startup=True
    try:
        while b:=front.recv(65536):
            back.sendall(b);pending+=b
            while True:
                if startup:
                    if len(pending)<4:break
                    n=struct.unpack('!i',pending[:4])[0]
                    if len(pending)<n:break
                    packet,pending=pending[:n],pending[n:]
                    code=struct.unpack('!i',packet[4:8])[0]
                    if code==196608:
                        # These probes use only routing labels/encoding, never credentials.
                        log('Startup '+repr(packet[8:]));startup=False
                    elif code==80877102:log('CancelRequest [key omitted]');return
                    else:log('EncryptionProbe '+str(code))
                else:
                    if len(pending)<5:break
                    tag=pending[:1];n=struct.unpack('!i',pending[1:5])[0]
                    if len(pending)<n+1:break
                    body,pending=pending[5:n+1],pending[n+1:]
                    if tag==b'Q':log('Query '+repr(body[:-1].decode()))
                    elif tag==b'P':log('Parse '+repr(body.split(b'\0')[1].decode()))
                    elif tag not in (b'B',b'E',b'S',b'D',b'C',b'H',b'X'):log('Tag '+repr(tag))
    except OSError:pass
    finally:
        back.close();front.close()
with socket.socket() as s:
    s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
    s.bind(('127.0.0.1',int(listen)));s.listen()
    while True:
        front,_=s.accept();threading.Thread(target=session,args=(front,),daemon=True).start()
