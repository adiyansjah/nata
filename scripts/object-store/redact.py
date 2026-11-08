#!/usr/bin/env python3
"""Literal runtime identifier redaction, shared by the real step and diagnostics."""
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import quote


def redactor(env):
    replacements = {}
    for key, label in [('NATA_OBJ_BUCKET', '<bucket>'), ('NATA_OBJ_PREFIX', '<prefix>'),
                       ('NATA_OBJ_PROJECT', '<project>'), ('GOOGLE_CLOUD_PROJECT', '<project>'),
                       ('NATA_OBJ_SERVICE_ACCOUNT', '<service-account>')]:
        value = env.get(key, '')
        if value:
            for variant in (value, quote(value, safe=''), quote(value, safe='/'), value.replace('/', r'\/')):
                replacements[variant.encode()] = label.encode()
    ordered = sorted(replacements.items(), key=lambda item: -len(item[0]))

    def apply(data):
        # One substitution pass: identifiers overlapping each other or placeholder
        # words must not corrupt the replacement text.
        if ordered:
            pattern = b'|'.join(re.escape(value) for value, _ in ordered)
            data = re.sub(pattern, lambda match: replacements[match[0]], data)
        data = re.sub(rb'[\w.+-]+@[\w.-]+\.gserviceaccount\.com', b'<service-account>', data)
        return re.sub(rb'\bhomelab-[0-9]+\b', b'<project>', data)
    return apply


def run(command):
    if not command or not os.environ.get('NATA_OBJ_BUCKET') or not os.environ.get('NATA_OBJ_PREFIX'):
        sys.exit('real-step requires a command, bucket and prefix')
    public = Path(os.environ['RESULTS']).resolve()
    private = Path(os.environ['WORK']).resolve() / 'object-store-private-results'
    private.mkdir(mode=0o700, parents=True, exist_ok=True)
    private.chmod(0o700)
    env = dict(os.environ, RESULTS=str(private), NATA_OSB_OUT=str(private / 'osb'),
               NATA_OBJ_PUBLIC_RESULTS=str(public))
    apply = redactor(env)
    child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
    try:
        for line in child.stdout:
            sys.stdout.buffer.write(apply(line))
            sys.stdout.buffer.flush()
        code = child.wait()
    finally:
        if child.poll() is None:
            child.terminate()
            child.wait()
        # Programs such as the spike write their own text/CSV artifacts. Keep
        # their private working copies for later steps; export only sanitized copies.
        for source in private.rglob('*'):
            if source.is_file() and not source.is_symlink():
                relative = apply(str(source.relative_to(private)).encode()).decode()
                target = public / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                with source.open('rb') as inp, target.open('wb') as out:
                    for line in inp:
                        out.write(apply(line))
    return code if code >= 0 else 128 - code


if __name__ == '__main__':
    if sys.argv[1:2] == ['--run']:
        sys.exit(run(sys.argv[2:]))
    apply = redactor(os.environ)
    for line in sys.stdin.buffer:
        sys.stdout.buffer.write(apply(line))
