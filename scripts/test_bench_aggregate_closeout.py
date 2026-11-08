"""Completion timing and safety controls; no benchmark fixtures required."""
import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('closeout', Path(__file__).with_name('bench-aggregate-closeout.py'))
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)


class Completion(unittest.TestCase):
    def test_short_child_completion_is_within_a_few_ms(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            for delay in (.001, .007, .013):
                ticks = []
                clock = time.perf_counter_ns
                def sample():
                    ticks.append(clock())
                    return ticks[-1]
                child = [sys.executable, '-c',
                         f'import time, os, sys; time.sleep({delay}); print(time.perf_counter_ns(), file=sys.stderr, flush=True); os._exit(0)']
                with patch.object(c.time, 'perf_counter_ns', side_effect=sample):
                    row = c.invoke(child, os.environ, base/'out', base/'err', 2)
                self.assertEqual(row['exit'], 0)
                self.assertLess(row['seconds'], .050)
                child_end = int((base/'err').read_text())
                # _exit excludes interpreter teardown from the exit-to-EOF
                # residual; the timed interval still includes startup and
                # sleep, independently exposes a polling wait's overshoot.
                self.assertLess(ticks[-1] - child_end, 5_000_000)
                self.assertGreaterEqual(ticks[-1], child_end)
                self.assertEqual(row['seconds'], (ticks[-1]-ticks[0])/1e9)

    def test_logs_output_counters_exit_and_timeout(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            child = [sys.executable, '-c',
                     'import sys; print("csv"); print("aggregate: groups=7", file=sys.stderr); sys.exit(3)']
            row = c.invoke(child, os.environ, base/'out', base/'err', 2)
            self.assertEqual((row['exit'], row['counters']), (3, {'groups': 7}))
            self.assertEqual((base/'out').read_text(), 'csv\n')
            row = c.invoke(['sh', '-c', 'echo waiting >&2; sleep 10'], os.environ,
                           base/'out', base/'err', .03)
            self.assertEqual(row['exit'], 124)
            self.assertIn('waiting', (base/'err').read_text())
            self.assertLess(row['seconds'], 1)  # inherited pipe closed by group kill


if __name__ == '__main__':
    unittest.main()
