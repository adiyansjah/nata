import unittest
import os
from pathlib import Path
import p16
if os.environ.get('P16_MUTANT'):
    old,new=os.environ['P16_MUTANT'].split('|||')
    source=Path(p16.__file__).read_text()
    assert source.count(old)==1
    exec(compile(source.replace(old,new),p16.__file__,'exec'),p16.__dict__)
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
