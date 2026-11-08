"""Exercise the actual prepared kept step without cloud or Cargo calls."""
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

repo = Path(__file__).resolve().parents[4]
command = next(line.split(' ', 1)[1] for line in (repo / 'bench/aggregate-wc2.steps').read_text().splitlines() if line.startswith('kept '))
base = '709674d47891c340df63273d4e8e8e415743f830'
for case, expected in [('missing', False), ('wrong-baseline', False), ('docs-only', False), ('kept', True)]:
    with tempfile.TemporaryDirectory(prefix='wc2-steps-') as tmp:
        tmp = Path(tmp)
        bundle = tmp / '.aggregate-wc2-inputs'
        if case != 'missing':
            bundle.mkdir()
            revisions = []
            for i in range(2):
                name = f'{i}.tar.gz'
                with tarfile.open(bundle / name, 'w:gz') as archive:
                    body = b'fn baseline() {}' if i == 0 or case == 'docs-only' else b'fn candidate() {}'
                    info = tarfile.TarInfo('crates/probe.rs')
                    info.size = len(body)
                    archive.addfile(info, io.BytesIO(body))
                revisions.append(dict(commit=base if case != 'wrong-baseline' else '0' * 40, archive=name))
            (bundle / 'manifest.json').write_text(json.dumps(dict(revisions=revisions)))
        env = dict(os.environ, PYTHONOPTIMIZE='1')
        run = subprocess.run(['bash', '-c', command], cwd=tmp, env=env, capture_output=True)
        assert (run.returncode == 0) == expected, (case, run.returncode, run.stderr.decode())
        print(f'{case}: expected {"accept" if expected else "reject"}')
print('prepared kept controls: PASS')
