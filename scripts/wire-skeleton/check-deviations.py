#!/usr/bin/env python3
"""Separate closeout verdict; never changes the original matrix or its exit."""
import argparse
import json
from pathlib import Path


def check(matrix, records, limits):
    indexed = {r['cell']: r for r in records}
    assert len(indexed) == len(records) == len(limits), 'duplicate/missing decisions'
    assert set(indexed) == set(limits), 'unapproved decision'
    for cell, record in indexed.items():
        for field in ('client', 'version', 'action', 'reason', 'source', 'substitute',
                      'limits', 'decision_owner', 'decision_date', 'tested_commit', 'revisit_trigger'):
            assert record.get(field), (cell, field)
        assert record['reason'] == limits[cell]['reason']
        assert record['source'] == limits[cell]['source']
        assert cell in matrix, ('missing matrix row', cell)
    for cell, result in matrix.items():
        if result == 'PASS' or result.startswith('PASS '):
            continue
        assert result.startswith('not run:') and cell in indexed, (cell, result)
    return 'PASS with nine accepted API deviations; original matrix remains non-PASS'


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('matrix', type=Path)
    a = ap.parse_args()
    here = Path(__file__).resolve().parent
    versions=json.loads(a.matrix.with_name('versions.json').read_text())
    for package,expected in [('psycopg','3.3.8'),('pgcli','4.7.1'),('adbc-driver-postgresql','1.13.0')]:
        assert versions[package]==expected, ('deviation version changed',package)
    import subprocess
    assert subprocess.check_output(['psql','--version'],text=True).split()[2]=='17.11', 'psql deviation must be revisited'
    assert 'connect: PASS driver=42.7.14' in a.matrix.with_name('jdbc.txt').read_text().splitlines(), 'pgjdbc deviation must be revisited'
    print(check(json.loads(a.matrix.read_text()), json.loads((here/'deviations.json').read_text()),
                json.loads((here/'api-limits.json').read_text())))
