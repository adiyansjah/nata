"""Exercise local budget selection, alternating twins and the pre-timing gate."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RUNNER = Path(__file__).with_name('bench-aggregate-perf2b-local.py')


class LocalBudgets(unittest.TestCase):
    def test_budgets_twins_and_value_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            data = base/'data'; data.mkdir(); (data/'lineitem.parquet').write_bytes(b'inert')
            body = '#!/bin/sh\necho a,b,c,d\necho 1,2,3,4\n'
            for name in ('before', 'after', 'duckdb'):
                binary = base/name; binary.write_text(body); binary.chmod(0o555)
            env = dict(os.environ, PATH=str(base)+os.pathsep+os.environ['PATH'])
            cmd = [sys.executable, str(RUNNER), 'inert', '--before', str(base/'before'),
                   '--after', str(base/'after'), '--data', str(data), '--out', str(base/'out'),
                   '--rounds', '2', '--queries', 'high_cardinality_tpch', '--budgets', '4294967296', '67108864']
            subprocess.run(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)
            samples = json.loads((base/'out/inert-samples.json').read_text())
            gates = json.loads((base/'out/inert-gates.json').read_text())
            self.assertEqual(len(gates), 12)
            self.assertEqual(len(samples), 24)
            self.assertEqual({s['budget'] for s in samples}, {4294967296, 67108864})
            for i in range(0, 24, 6):
                self.assertEqual([s['arm'] for s in samples[i:i+6]],
                                 ['before', 'after', 'control', 'control', 'after', 'before'])
            # A bad candidate must fail its DuckDB value gate before timing.
            (base/'after').chmod(0o755)
            (base/'after').write_text(body.replace('1,2,3,4', '9,2,3,4'))
            (base/'after').chmod(0o555)
            cmd[cmd.index('--out')+1] = str(base/'bad')
            result = subprocess.run(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b'RESULT CHANGED', result.stderr)
            self.assertFalse((base/'bad/inert-samples.json').exists())


if __name__ == '__main__':
    unittest.main()
