#!/usr/bin/env python3
"""Offline syntax and fail-closed controls for the prepared, never-launched job."""
import io, json, os, pathlib, subprocess, tarfile, tempfile
steps=pathlib.Path('bench/aggregate-wc1.steps')
lines=[s for s in steps.read_text().splitlines() if s and not s.startswith('#')]
for line in lines:
    name,command=line.split(' ',1)
    subprocess.run(['bash','-n','-c',command],check=True)
command=next(s.split(' ',1)[1] for s in lines if s.startswith('kept '))
base='2b7a415a183c3e06caa379c8c627abc4a43b89fe'
with tempfile.TemporaryDirectory() as temp:
    root=pathlib.Path(temp); bundle=root/'.aggregate-wc1-inputs';bundle.mkdir()
    def archive(name,rust,doc):
        with tarfile.open(bundle/name,'w:gz') as stream:
            for path,data in [('crates/inert/src/lib.rs',rust),('docs/inert.md',doc)]:
                data=data.encode();info=tarfile.TarInfo(path);info.size=len(data);stream.addfile(info,io.BytesIO(data))
    archive('baseline.tar.gz','fn before() {}','before')
    archive('docs.tar.gz','fn before() {}','different docs')
    archive('changed.tar.gz','fn changed() {}','after')
    def check(label,candidate,expected,baseline=base):
        revisions=[dict(commit=baseline,archive='baseline.tar.gz')]
        if candidate:revisions.append(dict(commit='f'*40,archive=candidate))
        (bundle/'manifest.json').write_text(json.dumps(dict(revisions=revisions)))
        result=subprocess.run(['bash','-c',command],cwd=root,env=dict(os.environ,PYTHONOPTIMIZE='1'),capture_output=True,text=True)
        assert (result.returncode==0)==expected,(label,result.stderr)
        if label=='docs-only':assert 'no kept Rust change' in result.stderr
        print(label,'accepted' if expected else 'rejected')
    check('docs-only','docs.tar.gz',False)
    check('changed-Rust','changed.tar.gz',True)
    check('no-candidate',None,False)
    check('wrong-baseline','changed.tar.gz',False,baseline='0'*40)
print('all step commands parse; controls pass with PYTHONOPTIMIZE=1')
