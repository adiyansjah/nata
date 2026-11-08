#!/usr/bin/env python3
"""Offline canaries for the real-bucket evidence boundary; no cloud requests."""
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
redactor = runpy.run_path(str(HERE / 'redact.py'))['redactor']


class RedactionTests(unittest.TestCase):
    def test_literal_encoded_and_overlapping_identifiers(self):
        apply = redactor({'NATA_OBJ_BUCKET': 'bucket', 'NATA_OBJ_PREFIX': 'prefix/a.b[0]'})
        self.assertEqual(apply(b'gs://bucket/prefix/a.b[0] prefix%2Fa.b%5B0%5D'),
                         b'gs://<bucket>/<prefix> <prefix>')
        self.assertEqual(apply(b'bucket'), b'<bucket>')

    def test_stdout_stderr_artifacts_and_failure_exit(self):
        with tempfile.TemporaryDirectory() as name:
            work = Path(name)
            results = work / 'results'
            results.mkdir()
            env_file = work / 'private.env'
            env_file.write_text("NATA_OBJ_BUCKET=bucket-canary\nNATA_OBJ_PREFIX=prefix-canary/path\n")
            env = dict(os.environ, WORK=name, RESULTS=str(results), NATA_OBJ_ENV_FILE=str(env_file))
            script = '''import os,pathlib,sys
text='cannot read: gs://'+os.environ['NATA_OBJ_BUCKET']+'/'+os.environ['NATA_OBJ_PREFIX']+'/x: the response is malformed'
print(text); print(text, file=sys.stderr)
p=pathlib.Path(os.environ['RESULTS'])/'osb'/'nested.txt'; p.parent.mkdir(parents=True); p.write_text(text)
sys.exit(37)
'''
            out = subprocess.run(['bash', str(HERE / 'real-step.sh'), 'python3', '-c', script],
                                 env=env, capture_output=True)
            self.assertEqual(out.returncode, 37, out.stderr)
            self.assertEqual(out.stdout.count(b'gs://<bucket>/<prefix>/x'), 2)
            for saved in results.rglob('*'):
                if saved.is_file():
                    data = saved.read_bytes()
                    self.assertNotIn(b'bucket-canary', data)
                    self.assertNotIn(b'prefix-canary', data)
                    self.assertIn(b'the response is malformed', data)
            self.assertTrue((results / 'osb/nested.txt').exists())
            # The leak-scan step must inspect exported evidence, not private state.
            out = subprocess.run(['bash', str(HERE / 'real-step.sh'), 'bash', '-c',
                                  'test "$RESULTS" != "$NATA_OBJ_PUBLIC_RESULTS"; test -f "$NATA_OBJ_PUBLIC_RESULTS/osb/nested.txt"'],
                                 env=env, capture_output=True)
            self.assertEqual(out.returncode, 0, out.stderr)
            rogue = results / 'rogue.txt'
            rogue.write_text('bucket-canary')
            scan = ['bash', str(HERE / 'real-step.sh'), 'bash',
                    str(HERE.parent.parent / 'spikes/object-store-baseline/run-reference.sh'), 'leakscan']
            out = subprocess.run(scan, env=env, capture_output=True)
            self.assertEqual(out.returncode, 1, out.stderr)
            self.assertIn(b'LEAK:', out.stdout)
            rogue.unlink()
            out = subprocess.run(scan, env=env, capture_output=True)
            self.assertEqual(out.returncode, 0, out.stderr)
            self.assertIn(b'leak scan clean', out.stdout)

    def test_diagnostic_records_only_allowlisted_header_values(self):
        module = runpy.run_path(str(HERE / 'gcs-diagnose.py'))
        class Response:
            status = 206
            def getheaders(self):
                return [('Content-Length', '8'), ('Content-Range', 'bytes 0-7/32'),
                        ('ETag', 'secret-validator'), ('X-Goog-Generation', 'secret-generation'),
                        ('Location', 'private-location'), ('Content-Encoding', 'identity')]
            def read(self, count):
                if getattr(self, 'done', False):
                    return b''
                self.done = True
                return b'12345678'
        class Connection:
            def __init__(self, *args, **kwargs): pass
            def request(self, *args, **kwargs): pass
            def getresponse(self): return Response()
            def close(self): pass
        import unittest.mock
        with unittest.mock.patch('http.client.HTTPSConnection', Connection):
            result, validators = module['probe']('example.invalid', '/x', 'GET', {})
        self.assertEqual(result['body_bytes'], 8)
        self.assertEqual(result['safe_headers']['content-range'], 'bytes 0-7/32')
        self.assertNotIn('secret-validator', str(result))
        self.assertNotIn('private-location', str(result))
        self.assertEqual(validators['etag'], 'secret-validator')


if __name__ == '__main__':
    unittest.main()
