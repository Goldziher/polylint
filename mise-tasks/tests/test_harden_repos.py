"""Check the hardening harness classifies final-dry-run output without running poly."""

import importlib.util
import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("harden_repos", ROOT / "mise-tasks/harden/repos.py")
assert _spec is not None and _spec.loader is not None
repos = importlib.util.module_from_spec(_spec)
sys.modules["harden_repos"] = repos
_spec.loader.exec_module(repos)


def run_with(rc: int, doc: dict) -> "repos.Run":
    return repos.Run(name="fmt-after", argv=[], rc=rc, stdout=json.dumps(doc))


class AfterDriftTest(unittest.TestCase):
    def test_per_file_errors_are_not_drift(self):
        doc = {"results": [{"path": "a.ex", "changed": False, "error": "boom"}], "errors": [{"path": "a.ex"}]}
        self.assertEqual(repos.after_drift(run_with(2, doc)), [])

    def test_would_change_files_are_drift(self):
        doc = {"results": [{"path": "a.rs", "changed": True}, {"path": "b.rs", "changed": False}]}
        self.assertEqual(repos.after_drift(run_with(1, doc)), ["a.rs"])


if __name__ == "__main__":
    unittest.main()
