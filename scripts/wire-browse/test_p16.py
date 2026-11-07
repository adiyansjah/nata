import unittest
from p16 import verdict,summary
class Gates(unittest.TestCase):
    def test_limits(self):
        ref=summary([1]*5,[1]*5)
        self.assertEqual(verdict(ref,summary([1.10]*5,[1.1]*5),ref),'MET')
        self.assertEqual(verdict(ref,summary([1.101]*5,[1.101]*5),ref),'NOT MET')
        self.assertEqual(verdict(ref,summary([1.20]*5,[1.2]*5),ref,'small'),'MET')
        self.assertEqual(verdict(ref,summary([1.201]*5,[1.201]*5),ref,'small'),'NOT MET')
        self.assertEqual(verdict(summary([1]*5,[1.051]*5),ref,ref),'UNRESOLVED')
if __name__=='__main__':unittest.main()
