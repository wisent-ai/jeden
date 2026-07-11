#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import shutil
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("run_migration_matrix.py")
SPEC = importlib.util.spec_from_file_location("run_migration_matrix", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = runner
SPEC.loader.exec_module(runner)


class MigrationMatrixTests(unittest.TestCase):
    def test_immutable_n_minus_2_through_future_fixture_matrix_is_complete(self) -> None:
        report, code = runner.evaluate()
        self.assertEqual(3, code)
        self.assertEqual("NotRun", report["classification"])
        self.assertEqual(28, len(report["fixtures"]))
        self.assertEqual({"Passed"}, {item["classification"] for item in report["fixtures"]})
        self.assertEqual(42, len(report["behaviors"]))
        self.assertEqual(set(runner.BEHAVIORS), {item["behavior"] for item in report["behaviors"]})
        self.assertEqual({"NotRun"}, {item["classification"] for item in report["behaviors"]})

    def test_changed_canonical_fixture_digest_fails_the_matrix_gate(self) -> None:
        matrix = json.loads(runner.DEFAULT_MATRIX.read_text(encoding="utf-8"))
        matrix["stores"][0]["sha256"]["n"] = "0" * 64
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "matrix.json"
            # Keep fixture-relative paths valid after relocating the manifest.
            for store in matrix["stores"]:
                store["path"] = str(runner.fixture_path(runner.DEFAULT_MATRIX, store, "{case}"))
            path.write_text(json.dumps(matrix), encoding="utf-8")
            report, code = runner.evaluate(path)
        self.assertEqual(1, code)
        self.assertEqual("Failed", report["classification"])
        failure = next(item for item in report["fixtures"] if item["store"] == "config" and item["case"] == "n")
        self.assertIn("digest changed", failure["detail"])

    def test_corrupt_and_truncated_inputs_are_detected_before_migration(self) -> None:
        matrix = json.loads(runner.DEFAULT_MATRIX.read_text(encoding="utf-8"))
        json_store = matrix["stores"][0]
        sqlite_store = next(store for store in matrix["stores"] if store["kind"] == "sqlite")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            corrupt_json = root / "corrupt.json"
            corrupt_json.write_bytes(b'{"schemaVersion":2,')
            with self.assertRaisesRegex(runner.MatrixError, "corrupt JSON fixture"):
                runner.inspect_fixture(corrupt_json, "json")

            source = runner.fixture_path(runner.DEFAULT_MATRIX, sqlite_store, "n")
            truncated_sqlite = root / "truncated.sqlite3"
            raw = source.read_bytes()
            truncated_sqlite.write_bytes(raw[: max(1, len(raw) // 3)])
            with self.assertRaisesRegex(runner.MatrixError, "corrupt SQLite fixture|SQLite integrity failure"):
                runner.inspect_fixture(truncated_sqlite, "sqlite")


if __name__ == "__main__":
    unittest.main()
