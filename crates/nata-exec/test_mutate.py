#!/usr/bin/env python3
"""Mutation-runner outcome classes: invalid runs never count as catches."""
import importlib.util
import re
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('exec_mutate', Path(__file__).with_name('mutate.py'))
mutate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mutate)


class Outcomes(unittest.TestCase):
    def test_all_live_edits_have_one_anchor(self):
        for name, (_, edits) in mutate.M.items():
            for relative, old, new in edits:
                with self.subTest(mutant=name, file=relative):
                    source = (Path(mutate.ROOT) / relative).read_text()
                    pattern = r"\s*".join(re.escape(c) for c in old if not c.isspace())
                    self.assertEqual(len(list(re.finditer(pattern, source))), 1)
                    self.assertNotEqual(re.sub(r"\s+", "", old), re.sub(r"\s+", "", new))

    def test_suite_outcomes_are_distinct(self):
        name = 'restart-cleanup-skipped'
        relative = mutate.M[name][1][0][0]
        original = (Path(mutate.ROOT) / relative).read_text()
        for code, output, expected in [(0,'','SURVIVED'),(99,'timeout','INVALID'),
                (101,'error[E0000]: could not compile','INVALID'),
                (101,'error: invalid manifest','INVALID'),
                (101,'rustc failed: signal: 9','INVALID'),
                (101,'Running tests/spill.rs; signal: 6 SIGABRT','CAUGHT'),
                (101,'test cleanup ... FAILED','CAUGHT')]:
            with self.subTest(code=code, output=output), tempfile.TemporaryDirectory() as temp:
                destination=Path(temp)/relative
                destination.parent.mkdir(parents=True)
                destination.write_text(original)
                with patch.object(mutate,'run_suite',return_value=(code,output)):
                    status,_=mutate.run_one(name,temp,temp)
                self.assertEqual(status,expected)
                self.assertEqual(destination.read_text(),original)


if __name__=='__main__':unittest.main()
