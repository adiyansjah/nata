import importlib.util
import json
from pathlib import Path
import unittest
p = Path(__file__).resolve().parent
s = importlib.util.spec_from_file_location('checker', p/'check-deviations.py')
c = importlib.util.module_from_spec(s)
s.loader.exec_module(c)
class Decisions(unittest.TestCase):
    def test_exact(self):
        records=json.loads((p/'deviations.json').read_text())
        limits=json.loads((p/'api-limits.json').read_text())
        matrix={k:'not run: API limit' for k in limits}
        c.check(matrix,records,limits)
        for cell in matrix:
            with self.assertRaises(AssertionError):
                c.check(matrix,[r for r in records if r['cell']!=cell],limits)
            with self.assertRaises(AssertionError):
                c.check(dict(matrix,**{cell:'FAIL'}),records,limits)
        with self.assertRaises(AssertionError):
            c.check(dict(matrix,unknown='not run: absent'),records,limits)
if __name__=='__main__': unittest.main()
