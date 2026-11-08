import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('precheck', Path(__file__).with_name('profile-precheck.py'))
p = importlib.util.module_from_spec(spec); spec.loader.exec_module(p)


class Precheck(unittest.TestCase):
    def test_known_chain_and_loss(self):
        self.assertEqual(p.useful('nata_profile_leaf\n nata_profile_parent\n')['useful_chains'], 1)
        self.assertEqual(p.useful('nata_profile_leaf\n\nnata_profile_parent\n')['useful_chains'], 0)
        self.assertEqual(p.useful('[unknown]\nPERF_RECORD_LOST')['lost_records'], 1)
        self.assertEqual(p.useful('[unknown]\n')['unknown_frames'], 1)


class Trials(unittest.TestCase):
    def test_all_events_retains_failures_and_successes(self):
        import json
        import subprocess
        import tempfile
        from unittest.mock import patch
        for works in (True, False):
            with tempfile.TemporaryDirectory() as temporary:
                out = Path(temporary)/'precheck'
                def fake(argv, **kw):
                    if argv[:2] == ['perf', 'record']:
                        event = argv[argv.index('-e')+1]
                        success = works and event != 'cpu-clock:u'
                        if success:
                            Path(argv[-2]).write_text('1.00 0.99 0.01')
                        return subprocess.CompletedProcess(argv, 0 if success else 1, '', '' if success else 'event rejected')
                    if argv[:2] == ['perf', 'script']:
                        return subprocess.CompletedProcess(argv, 0, ('nata_profile_leaf\n nata_profile_parent\n\n')*6, '')
                    return subprocess.CompletedProcess(argv, 0, '', '')
                with patch.object(p.shutil, 'which', return_value='/tool'), patch.object(p.subprocess, 'run', side_effect=fake):
                    self.assertEqual(p.run(out, all_events=True), 0 if works else 1)
                result=json.loads((out/'precheck.json').read_text())
                self.assertEqual([a['event'] for a in result['attempts']], list(p.EVENTS))
                self.assertEqual(result['working_events'], list(p.EVENTS[1:]) if works else [])
