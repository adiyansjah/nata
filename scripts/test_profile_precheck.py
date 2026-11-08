import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('precheck', Path(__file__).with_name('profile-precheck.py'))
p = importlib.util.module_from_spec(spec); spec.loader.exec_module(p)


class Precheck(unittest.TestCase):
    def test_known_chain_and_loss(self):
        self.assertEqual(p.useful('nata_profile_leaf\n nata_profile_parent\n')['useful_chains'], 1)
        self.assertEqual(p.useful('nata_profile_leaf\n\nnata_profile_parent\n')['useful_chains'], 0)
        self.assertEqual(p.useful('[unknown]\nPERF_RECORD_LOST')['lost_records'], 1)
        self.assertEqual(p.useful('[unknown]\n')['unknown_frames'], 1)
