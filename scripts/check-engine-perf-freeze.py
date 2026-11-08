#!/usr/bin/env python3
"""Validate an owner-supplied package-0 freeze before any Nata reference timing."""
import hashlib
import json
from pathlib import Path


def validate(path):
    f=json.loads(path.read_text())
    assert f['owner_accepted'] is True and f['policy']=='0008-v1'
    assert f['slice5_sha']=='bb946ab'
    assert f['package0_off_cost_rule']=='within-twin-noise-v1'
    assert f['budgets']==[4<<30,64<<20,256<<20]
    assert f['threads']==[1,4,8]
    assert f['fixture_hashes'] and f['successor_fixture_hashes']
    manifest=path.parent/f['manifest_path']
    assert hashlib.sha256(manifest.read_bytes()).hexdigest()==f['manifest_sha256']
    assert f['source_sha']==(path.parent/'.engine-perf-package0-commit').read_text().strip()
    print('PASS owner freeze, manifest hash, source identity, topology and budgets')


if __name__=='__main__':
    import argparse
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('path',type=Path)
    validate(p.parse_args().path)
