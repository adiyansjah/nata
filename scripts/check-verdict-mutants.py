#!/usr/bin/env python3
"""Small deterministic mutation screen for the Python verdict engine (no deps)."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent
MUTANTS = {
    'relax_frozen_q1': ('return 1.5', 'return 2.0'),
    'accept_slowdown': ("verdict = 'MET' if median_ok and sum(within) >= 4", "verdict = 'MET' if True"),
    'ignore_noise': ('max(noises.values()) > tolerance', 'False'),
    'three_of_five': ('sum(within) >= 4', 'sum(within) >= 3'),
    'ignore_stats': ('if require_stats:', 'if False:'),
    'ignore_drops': ("if record.get('dropped_events'):", 'if False:'),
    'reverse_floor': ('r >= limit if floor else r <= limit', 'r <= limit if floor else r <= limit'),
    'pair_arrival_order': ("sorted(rows, key=lambda s: s['round'])", 'rows'),
    'ignore_reference_exit': ("not complete_reference or any(s['exit'] for s in samples if s['variant'] == reference)", 'False'),
}
with tempfile.TemporaryDirectory() as directory:
    work = Path(directory)
    shutil.copy(ROOT/'test_bench_verdict.py', work)
    source = (ROOT/'bench_verdict.py').read_text()
    for name, (old, new) in MUTANTS.items():
        assert old in source, name
        (work/'bench_verdict.py').write_text(source.replace(old, new, 1))
        shutil.rmtree(work/'__pycache__', ignore_errors=True)
        run = subprocess.run([sys.executable, '-B', '-m', 'unittest', 'test_bench_verdict'], cwd=work, capture_output=True, text=True)
        print(name, 'CAUGHT' if run.returncode else 'MISSED')
        if not run.returncode:
            raise SystemExit(1)
