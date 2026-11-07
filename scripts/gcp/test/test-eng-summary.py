"""A failed sub-check must fail the collected engineering job, after printing all checks."""
from pathlib import Path
import os
import subprocess
import tempfile

job = (Path(__file__).resolve().parents[1] / 'job.sh').read_text()
sanitizer = next(line for line in job.splitlines() if 'run "ec-$san"' in line)
assert '--workspace' in sanitizer and '--no-fail-fast' in sanitizer
assert '--skip' not in sanitizer
assert 'NATA_RSS_BINARY=' in sanitizer and 'NATA_INSTRUMENTED=1' in sanitizer
assert 'run ec-fixtures' in job
line = next(line for line in job.splitlines() if 'run ec-summary bash -c' in line)
command = line.split("bash -c '", 1)[1].rsplit("'", 1)[0]
with tempfile.TemporaryDirectory() as tmp:
    env = dict(os.environ, RESULTS=tmp)
    a = Path(tmp)/'eng-address-exit.txt'
    b = Path(tmp)/'eng-thread-exit.txt'
    a.write_text('exit status: 0\n')
    b.write_text('exit status: 0\n')
    assert subprocess.run(['bash', '-c', command], env=env, capture_output=True).returncode == 0
    b.write_text('exit status: 101\n')
    result = subprocess.run(['bash', '-c', command], env=env, capture_output=True, text=True)
    assert result.returncode != 0 and 'eng-address' in result.stdout and 'eng-thread' in result.stdout
    b.write_text('')
    assert subprocess.run(['bash', '-c', command], env=env, capture_output=True).returncode != 0
print('engineering summary: pass, planted failure and empty result caught')
