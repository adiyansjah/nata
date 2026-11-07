#!/usr/bin/env python3
"""Record the code-only Git tree shipped by pgwire clean-VM checks.
Results/docs/baseline CSVs are excluded to avoid a circular measured hash.
"""
import os
from pathlib import Path
import subprocess as sp
import tempfile
root=Path(__file__).resolve().parents[1]
os.chdir(root)
paths=sp.check_output(['git','ls-files','--cached','--others','--exclude-standard','-z']).decode().split('\0')
roots={'crates','scripts','check','.cargo'}
exts={'.rs','.py','.sh','.java','.toml','.lock','.env','.sql'}
paths=sorted(p for p in set(paths) if p and Path(p).is_file() and (p in ('Cargo.toml','Cargo.lock','rust-toolchain.toml','deny.toml') or (p.split('/')[0] in roots and Path(p).suffix in exts)))
with tempfile.TemporaryDirectory() as work:
    env=dict(os.environ,GIT_INDEX_FILE=str(Path(work)/'index'))
    sp.run(['git','read-tree','--empty'],env=env,check=True)
    sp.run(['git','add','--']+paths,env=env,check=True)
    tree=sp.check_output(['git','write-tree'],env=env,text=True).strip()
    manifest=sp.check_output(['git','ls-files','--stage'],env=env,text=True)
prefix=f'Code-only Git tree {tree}\nRoots: crates, scripts, check, Cargo.toml, Cargo.lock, rust-toolchain.toml, .cargo, deny.toml\nExtensions: rs py sh java toml lock env sql\n'
(root/'crates/nata-bench/pgwire-source-tree.txt').write_text(tree+'\n')
(root/'crates/nata-bench/pgwire-source-manifest.txt').write_text(prefix+manifest)
print(tree)
