"""Independent arithmetic boundaries; excess, cleanup and FD are strict gates."""
import unittest
import os
from pathlib import Path
import resources
source=Path(resources.__file__).read_text()
if os.environ.get('RESOURCE_MUTANT'):
    old,new=os.environ['RESOURCE_MUTANT'].split('|||')
    assert source.count(old)==1
    exec(compile(source.replace(old,new),resources.__file__,'exec'),resources.__dict__)
from resources import budget, MiB, peak_ok, cleanup_ok
class Budget(unittest.TestCase):
    def test_frozen_terms(self):
        self.assertEqual(budget(7*MiB,64,32*MiB),1159*MiB)
        self.assertEqual(budget(7*MiB,1,32*MiB),int(119.5*MiB))
    def test_excess(self):
        self.assertTrue(peak_ok(10,10))
        self.assertFalse(peak_ok(11,10))
        self.assertTrue(cleanup_ok(64*MiB,0,7,7))
        self.assertFalse(cleanup_ok(64*MiB+1,0,7,7))
        self.assertFalse(cleanup_ok(0,0,8,7))
if __name__=='__main__':unittest.main()
