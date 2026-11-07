import socket,threading,unittest
from protocol import msg,frame
class Framing(unittest.TestCase):
 def test_fragmentation(self):
  x,y=socket.socketpair()
  def writer():
   for b in msg('D',b'abc'): y.sendall(bytes([b]))
   y.close()
  t=threading.Thread(target=writer);t.start()
  self.assertEqual(frame(x),('D',b'abc'));t.join();x.close()
 def test_bad_length(self):
  x,y=socket.socketpair();y.sendall(b'D\0\0\0\3')
  with self.assertRaises(AssertionError):frame(x)
  x.close();y.close()
if __name__=='__main__':unittest.main()
