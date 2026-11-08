"""Failing shipment/freeze controls; no cloud calls or Rust builds."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('freeze', ROOT/'scripts/check-engine-perf-freeze.py')
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)


class Package0(unittest.TestCase):
    def test_freeze_requires_owner_source_manifest_and_frozen_budgets(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            (root/'reference.json').write_text('{}')
            (root/'.engine-perf-package0-commit').write_text('a'*40)
            record=dict(owner_accepted=True,policy='0008-v1',slice5_sha='bb946ab',
                        source_sha='a'*40,package0_off_cost_rule='within-twin-noise-v1',
                        budgets=[4<<30,64<<20,256<<20],threads=[1,4,8],fixture_hashes={'q1':'hash'},
                        successor_fixture_hashes={'sort':'hash'},manifest_path='reference.json',
                        manifest_sha256=hashlib.sha256(b'{}').hexdigest())
            path=root/'freeze.json';path.write_text(json.dumps(record));f.validate(path)
            for key,value in [('owner_accepted',False),('source_sha','b'*40),('budgets',[8<<30]),
                              ('threads',[1,8]),('manifest_sha256','bad'),('successor_fixture_hashes',{}),
                              ('package0_off_cost_rule','relaxed')]:
                bad=copy.deepcopy(record);bad[key]=value;path.write_text(json.dumps(bad))
                with self.assertRaises(AssertionError):f.validate(path)

    def test_steps_gate_and_use_immutable_source_and_binaries(self):
        for name in ('precheck','baseline'):
            lines=(ROOT/f'bench/engine-perf-package0-{name}.steps').read_text().splitlines()
            self.assertEqual(lines[0],'# gate-exits')
            commands={line.split(' ',1)[0]:line.split(' ',1)[1] for line in lines if line and not line.startswith('#')}
            self.assertIn('sha256sum -c',commands['prepare'])
            self.assertIn('--all-events',commands['profile'])
            if name=='baseline':
                self.assertLess(list(commands).index('freeze'),list(commands).index('off-cost'))
                self.assertIn('nata_stats_disabled',commands['build-without'])
                self.assertIn('--settled',commands['off-cost'])
                for arm in ('unpinned','pinned'):
                    self.assertIn('--early-sf10',commands['sf-ten-'+arm])
                    self.assertIn('--affinity '+arm,commands['sf-ten-'+arm])
        startup=(ROOT/'scripts/gcp/vm-startup.sh').read_text()
        self.assertIn('= heavy ] && grep -q \'profile-precheck.py\'',startup)
        self.assertIn('install -y -qq linux-perf',startup)


class OffCostHarness(unittest.TestCase):
    def test_alternation_twins_disabled_environment_and_negative_control(self):
        import argparse
        import contextlib
        import io
        from unittest.mock import patch
        spec=importlib.util.spec_from_file_location('offcost',ROOT/'scripts/bench-stats-off-cost.py')
        b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
        for settled,slowdown in ((False,False),(True,False),(True,True)):
            with tempfile.TemporaryDirectory() as temporary:
                root=Path(temporary);data=root/'data';data.mkdir();(data/'lineitem.parquet').write_bytes(b'fixture')
                for label in ('without','disabled'):
                    (root/label).write_bytes(label.encode());(root/label).chmod(0o444)
                calls=[]
                def fake(argv,env,output,log,timeout):
                    self.assertNotIn('NATA_STATS_JSON',env)
                    self.assertNotIn('NATA_QUERY_STATS',env)
                    output.write_text('value\n')
                    label=Path(str(argv[0])).name
                    if log.name[0].isdigit():calls.append((label,log.name))
                    ns=1_200_000_000 if slowdown and label=='disabled' else 1_000_000_000
                    return dict(exit=0,seconds=ns/1e9,elapsed_ns=ns,stats=None)
                a=argparse.Namespace(without=root/'without',disabled=root/'disabled',data=data,out=root/'out',
                                     queries=['q1'],threads=[1],budgets=[64<<20],settled=settled)
                with patch.object(b,'invoke',side_effect=fake),patch.object(b.h,'rows',return_value={'row':1}),patch.object(b.h,'equal_values',return_value=True),patch.object(b.subprocess,'check_output',return_value='topology'),patch.object(b.subprocess,'run',return_value=argparse.Namespace(returncode=0,stdout='settled',stderr='')),patch.object(b.os,'cpu_count',return_value=8),patch.object(b.platform,'machine',return_value='x86_64'),contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(b.run(a),int(settled and slowdown))
                summary=json.loads((a.out/'summary.json').read_text())[0]
                self.assertEqual(summary['verdict'],'NOT MET' if slowdown else ('MET' if settled else 'UNRESOLVED'))
                self.assertEqual([v for v,_ in calls[:8]],['without','without','disabled','disabled','disabled','disabled','without','without'])
                self.assertEqual(len(json.loads((a.out/'samples.json').read_text())),20)
