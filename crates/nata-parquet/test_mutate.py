"""Negative controls for the mutation runner's compile/test distinction."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("parquet_mutator", Path(__file__).with_name("mutate.py"))
mutator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mutator)


class Classification(unittest.TestCase):
    def classify(self, compile_code, compile_output, group_ok, group_output):
        name = "adapter-reads-deprecated-min-max"
        file, old, _ = mutator.M[name]
        with tempfile.TemporaryDirectory() as scratch:
            base = Path(scratch)
            source = base / file
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text(old)
            with patch.object(mutator, "stage", return_value=(base, base)), \
                 patch.object(mutator, "cargo", return_value=(compile_code, compile_output)) as compile_call, \
                 patch.object(mutator, "run_groups", return_value=(group_ok, group_output, "test-group")):
                result = mutator.run_one((name, scratch, False, 0))
                self.assertIn("--lib", compile_call.call_args.args[-1])
                self.assertIn("--no-run", compile_call.call_args.args[-1])
                return result[1]

    def test_library_compile_failure_is_invalid(self):
        self.assertEqual(self.classify(101, "error[E0308]: mismatch", False, ""), "INVALID")

    def test_later_group_compile_failure_is_invalid(self):
        self.assertEqual(self.classify(0, "", False, "error: could not compile test target"), "INVALID")

    def test_real_assertion_failure_is_caught(self):
        self.assertEqual(self.classify(0, "", False, "test catches_bad_stats ... FAILED"), "CAUGHT")

    def test_passing_tests_mean_survived(self):
        self.assertEqual(self.classify(0, "", True, "test result: ok"), "SURVIVED")


if __name__ == "__main__":
    unittest.main()
