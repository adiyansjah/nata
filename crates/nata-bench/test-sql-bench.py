#!/usr/bin/env python3
"""A restarted benchmark must not publish a previous incarnation's summary.
Run with the benchmark's Python environment (duckdb installed).
"""
from pathlib import Path
import os
import subprocess
import sys
import tempfile

script=Path(__file__).with_name('sql-bench.py').resolve()
with tempfile.TemporaryDirectory() as tmp:
    root=Path(tmp)
    work=root/'work'; out=root/'out'
    work.mkdir(); out.mkdir()
    (work/'small').mkdir(); (work/'small/stale.parquet').write_text('stale')
    (work/'sorted.parquet').write_text('stale')
    for name in ['runs.csv','medians.csv','summary.md','limits.txt','settle.txt','reports.txt']:
        (out/name).write_text('STALE')
    quiet=root/'quiet'; quiet.write_text('0.01 0.01 0.01 1/1 1\n')
    env=dict(os.environ,SETTLE_LOADAVG=str(quiet))
    # Fail the new incarnation at its first planner invocation, before any
    # complete summary is written. No cloud or data generation is involved.
    result=subprocess.run([sys.executable,str(script),str(root/'missing-nata'),
                           str(root/'missing-planner'),str(root/'sf1'),str(root/'sf10'),
                           str(work),str(out)],env=env,capture_output=True,text=True)
    assert result.returncode != 0
    for name in ['medians.csv','summary.md','limits.txt']:
        assert not (out/name).exists(), name
    assert len((out/'runs.csv').read_text().splitlines()) == 1
    assert 'STALE' not in (out/'settle.txt').read_text()
    assert not (work/'small').exists() and not (work/'sorted.parquet').exists()
print('SQL benchmark: failing restart clears stale summaries, samples and owned fixtures')
