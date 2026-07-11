#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("run_behavioral_gates.py")
SPEC = importlib.util.spec_from_file_location("run_behavioral_gates", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = runner
SPEC.loader.exec_module(runner)


class BehavioralGateTests(unittest.TestCase):
    def test_symbol_presence_never_overrides_failed_behavior(self) -> None:
        report = runner.classify(runner.CONTRACTS / "negative-symbol-behavior.json", now_ms=50)
        self.assertEqual("Failed", report["classification"])
        self.assertIn("behavioral scenario outcome is failed", report["reasons"])
        self.assertTrue(any("execution attempt outcome" in reason for reason in report["reasons"]))

    def test_expired_evidence_is_rejected_even_when_execution_passed(self) -> None:
        report = runner.classify(runner.CONTRACTS / "stale-evidence.json", now_ms=22)
        self.assertEqual("Failed", report["classification"])
        self.assertIn("behavioral evidence expired at 21", report["reasons"])

    def test_fresh_coherent_passed_evidence_is_accepted(self) -> None:
        document = json.loads((runner.CONTRACTS / "stale-evidence.json").read_text(encoding="utf-8"))
        document["expiresAt"] = 100
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "fresh.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            report = runner.classify(path, now_ms=50)
        self.assertEqual("Passed", report["classification"])
        self.assertEqual([], report["reasons"])

    def test_inconsistent_attempt_sequence_fails_closed(self) -> None:
        document = json.loads((runner.CONTRACTS / "stale-evidence.json").read_text(encoding="utf-8"))
        document["expiresAt"] = 100
        document["attempts"][0]["attempt"] = 2
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "bad-attempt.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            report = runner.classify(path, now_ms=50)
        self.assertEqual("Failed", report["classification"])
        self.assertIn("attempt numbers must be contiguous from one", report["reasons"])


if __name__ == "__main__":
    unittest.main()
