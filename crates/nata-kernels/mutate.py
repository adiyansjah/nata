#!/usr/bin/env python3
"""Exact aggregate mutants, private crate copies; INVALID never counts.
Full sweeps: parent heavy job. Local trial: --only average-tie --jobs 1.
"""
import argparse
import concurrent.futures
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[2]
CRATE = ROOT / 'crates/nata-kernels'
MUTANTS = {
    'sum-overflow-not-checked': ('if !fits(value, precision) {', 'if false {'),
    'average-tie-toward-zero': ('if twice >= denominator {', 'if twice > denominator {'),
    'merge-count-guard-removed': ('let count = checked_count(self.count, other.count)?;', 'let count = self.count;'),
    'partial-premature-narrowing': ('let count = checked_count(self.count, 1)?;', 'self.decimal_sum(38)?;\n        let count = checked_count(self.count, 1)?;'),
    'merge-double-counts': ('.checked_add(other.sum)', '.checked_add(other.sum).and_then(|v| v.checked_add(other.sum))'),
    'spill-wide-state-ignored': ('let sum = i256::from_le_bytes(bytes);', 'let _ = bytes;\n        let sum = i256::ZERO;'),
}

def command(args, cwd, env):
    try:
        r = subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True, timeout=180)
        return r.returncode, r.stdout + r.stderr
    except subprocess.TimeoutExpired:
        return 124, 'timeout'

def run(name):
    with tempfile.TemporaryDirectory(prefix='nata-aggregate-mutant-', dir=ROOT / 'target') as temporary:
        path = Path(temporary)
        for directory in ['src', 'tests']:
            shutil.copytree(CRATE / directory, path / directory)
        # Keep the real dependency/lint declarations, detached from the workspace.
        manifest = (CRATE / 'Cargo.toml').read_text()
        replacements = {
            'version.workspace = true': 'version = "0.0.0"',
            'publish.workspace = true': 'publish = false',
            'edition.workspace = true': 'edition = "2024"',
            'license.workspace = true': 'license = "Apache-2.0"',
            'rust-version.workspace = true': 'rust-version = "1.99"',
        }
        manifest = '\n'.join(replacements.get(line, line) for line in manifest.splitlines())
        (path / 'Cargo.toml').write_text(manifest + '\n[workspace]\n')
        shutil.copy(CRATE / 'clippy.toml', path / 'clippy.toml')
        shutil.copy(ROOT / 'Cargo.lock', path / 'Cargo.lock')
        source = path / 'src/aggregate.rs'
        text = source.read_text()
        if name != 'control':
            old, new = MUTANTS[name]
            if text.count(old) != 1:
                return name, 'INVALID', 'anchor count'
            source.write_text(text.replace(old, new))
        env = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / 'target' / 'aggregate-mutant-build' / str(threading.get_ident())), CARGO_BUILD_JOBS='2')
        rc, out = command(['cargo', 'test', '--offline', '--test', 'aggregate', '--no-run'], path, env)
        if rc:
            return name, 'INVALID', out[-1500:]
        rc, out = command(['cargo', 'test', '--offline', '--test', 'aggregate', '--', '--test-threads=1'], path, env)
        if name == 'control':
            return name, 'CONTROL_OK' if rc == 0 else 'CONTROL_FAILED', out[-1000:]
        # A failed assertion/test or an execution timeout counts; harness failures don't.
        if rc == 124 or ('test result: FAILED' in out and 'panicked at' in out):
            return name, 'CAUGHT', out[-1000:]
        return name, 'SURVIVED' if rc == 0 else 'INVALID', out[-1000:]

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--only', choices=list(MUTANTS))
    p.add_argument('--control', action='store_true')
    p.add_argument('--jobs', type=int, default=1)
    p.add_argument('--list', action='store_true')
    args = p.parse_args()
    if args.list:
        print('\n'.join(MUTANTS))
        return 0
    baseline = run('control')
    print(f'{baseline[0]}: {baseline[1]}\n{baseline[2]}', flush=True)
    if baseline[1] != 'CONTROL_OK':
        return 2
    if args.control:
        return 0
    names = [args.only] if args.only else list(MUTANTS)
    if args.jobs == 1:
        results = [run(name) for name in names]
    else:
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
            results = list(pool.map(run, names))
    for name, verdict, detail in results:
        print(f'{name}: {verdict}\n{detail}', flush=True)
    return int(any(v not in ('CAUGHT', 'CONTROL_OK') for _, v, _ in results))

if __name__ == '__main__':
    raise SystemExit(main())
