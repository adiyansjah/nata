"""Independent arithmetic boundaries; excess, cleanup and FD are strict gates."""
import unittest
from resources import budget, MiB
class Budget(unittest.TestCase):
    def test_frozen_terms(self):
        self.assertEqual(budget(7*MiB,64,32*MiB),1159*MiB)
        self.assertEqual(budget(7*MiB,1,32*MiB),int(119.5*MiB))
if __name__=='__main__':unittest.main()
