"""Replay a synthetic fixture against a matching PostgreSQL build (port 55432).
Run fixture.sql first. Backend key material is never part of fixtures.
"""
import json,sys,socket,time
from protocol import Conn,i
c=Conn()
for step in json.load(open(sys.argv[1]))['steps']:
    front=step['frontend']; front=front if isinstance(front[0],list) else [front]
    for t,b in front:c.send(t,bytes.fromhex(b))
    if step.get('concurrent'):
        time.sleep(.05);s=socket.create_connection(('127.0.0.1',55432));s.sendall(i(16)+i(80877102)+c.key);s.close()
    if 'backend' in step:
        expected=step['backend'];actual=c.until((expected[-1][0],))
        assert actual==expected,(actual,expected)
c.close(); print('byte replay PASS')
