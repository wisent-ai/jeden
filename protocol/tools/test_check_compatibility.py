#!/usr/bin/env python3
"""Focused stdlib regression tests for check_compatibility.py."""

from __future__ import annotations

import json
import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("check_compatibility.py")
SPEC = importlib.util.spec_from_file_location("check_compatibility", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
checker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = checker
SPEC.loader.exec_module(checker)


class SchemaSubsetTests(unittest.TestCase):
    def test_all_of_keeps_sibling_object_validation(self) -> None:
        schema = {
            "type": "object",
            "properties": {"name": {"type": "string", "minLength": 1}},
            "required": ["name"],
            "additionalProperties": False,
            "allOf": [{}],
        }
        failures = checker._validate({"name": "", "extra": 1}, schema, schema, "fixture")
        self.assertTrue(any("minLength" in item for item in failures), failures)
        self.assertTrue(any("unknown property 'extra'" in item for item in failures), failures)

    def test_conditional_replay_params_are_enforced(self) -> None:
        schema = {
            "type": "object",
            "properties": {"method": {"type": "string"}, "params": {}},
            "required": ["method", "params"],
            "allOf": [{
                "if": {"properties": {"method": {"const": "session.replay"}}},
                "then": {"properties": {"params": {
                    "type": "object",
                    "properties": {"sessionId": {"type": "string", "minLength": 1}},
                    "required": ["sessionId"],
                    "additionalProperties": False,
                }}},
            }],
        }
        failures = checker._validate(
            {"method": "session.replay", "params": {"sessionId": "", "typo": 1}},
            schema,
            schema,
            "fixture",
        )
        self.assertTrue(any("minLength" in item for item in failures), failures)
        self.assertTrue(any("unknown property 'typo'" in item for item in failures), failures)

    def test_local_ref_and_discriminator(self) -> None:
        schema = {
            "$defs": {"tag": {"const": "request"}},
            "type": "object",
            "properties": {"type": {"$ref": "#/$defs/tag"}},
            "required": ["type"],
            "additionalProperties": False,
        }
        self.assertEqual([], checker._validate({"type": "request"}, schema, schema, "fixture"))
        failures = checker._validate({"type": "response"}, schema, schema, "fixture")
        self.assertTrue(any("expected constant 'request'" in item for item in failures), failures)


class CompatibilityTests(unittest.TestCase):
    def test_repository_manifest_is_deterministic(self) -> None:
        root = Path(__file__).resolve().parents[2]
        first_errors, first_manifest = checker.check(root)
        second_errors, second_manifest = checker.check(root)
        self.assertEqual(first_errors, second_errors)
        self.assertEqual(
            json.dumps(first_manifest, sort_keys=True, separators=(",", ":")),
            json.dumps(second_manifest, sort_keys=True, separators=(",", ":")),
        )

    def test_sdk_constant_drift_has_actionable_language_diagnostic(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            rust = root / "rust" / "sdk"
            typescript = root / "packages" / "sdk-typescript"
            python = root / "python" / "jeden_sdk"
            rust.mkdir(parents=True)
            typescript.mkdir(parents=True)
            python.mkdir(parents=True)
            rust_fields = " ".join(checker.RUST_FIELDS.values())
            (rust / "lib.rs").write_text(
                '#[serde(rename_all = "camelCase")]\n'
                'pub const PROTOCOL_VERSION: &str = "jeden.session.v1";\n'
                f'// {rust_fields}\n',
                encoding="utf-8",
            )
            camel_fields = " ".join(checker.CAMEL_FIELDS)
            (typescript / "index.ts").write_text(
                'export const PROTOCOL_VERSION = "jeden.session.v1";\n'
                f'// {camel_fields}\n',
                encoding="utf-8",
            )
            (python / "__init__.py").write_text(
                'PROTOCOL_VERSION = "jeden.session.v2"\n'
                f'# {camel_fields}\n',
                encoding="utf-8",
            )
            errors: list[str] = []
            checker._check_sdks(root, errors)
            self.assertTrue(
                any(
                    item == "python SDK: protocol constant value 'jeden.session.v1' not found"
                    for item in errors
                ),
                errors,
            )
            self.assertFalse(any(item.startswith("rust SDK:") for item in errors), errors)
            self.assertFalse(any(item.startswith("typescript SDK:") for item in errors), errors)


if __name__ == "__main__":
    unittest.main()
