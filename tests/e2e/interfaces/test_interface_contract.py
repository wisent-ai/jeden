#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("run_interface_contract.py")
SPEC = importlib.util.spec_from_file_location("run_interface_contract", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = runner
SPEC.loader.exec_module(runner)


class InterfaceContractTests(unittest.TestCase):
    def test_all_representative_adapter_ledgers_are_semantically_equivalent(self) -> None:
        report, code = runner.evaluate(runner.DEFAULT_SCENARIO, runner.DEFAULT_CAPTURES)
        self.assertEqual(0, code)
        self.assertEqual("Passed", report["classification"])
        self.assertEqual(list(runner.ADAPTERS), [item["adapter"] for item in report["adapters"]])
        self.assertEqual({"Passed"}, {item["classification"] for item in report["adapters"]})

    def test_divergent_terminal_outcome_fails_the_behavioral_gate(self) -> None:
        fixture = runner.FIXTURES / "divergent-headless-v1.json"
        report, code = runner.evaluate(runner.DEFAULT_SCENARIO, fixture)
        self.assertEqual(1, code)
        self.assertEqual("Failed", report["classification"])
        headless = next(item for item in report["adapters"] if item["adapter"] == "headless")
        self.assertEqual("Failed", headless["classification"])
        self.assertNotEqual(headless["expectedSha256"], headless["actualSha256"])

    def test_missing_runtime_capture_is_external_blocked_not_passed(self) -> None:
        document = json.loads(runner.DEFAULT_CAPTURES.read_text(encoding="utf-8"))
        document["captures"] = [item for item in document["captures"] if item["adapter"] != "acp"]
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "captures.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            report, code = runner.evaluate(runner.DEFAULT_SCENARIO, path)
        self.assertEqual(2, code)
        self.assertEqual("ExternalBlocked", report["classification"])
        acp = next(item for item in report["adapters"] if item["adapter"] == "acp")
        self.assertEqual({"adapter": "acp", "classification": "ExternalBlocked", "prerequisites": ["acp capture artifact"]}, acp)


if __name__ == "__main__":
    unittest.main()
