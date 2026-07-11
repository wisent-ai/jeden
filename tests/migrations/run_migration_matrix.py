#!/usr/bin/env python3
"""Validate immutable migration fixtures and classify unavailable runtime exercises."""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
DEFAULT_MATRIX = HERE / "fixtures" / "matrix-v1.json"
CASES = ("n-2", "n-1", "n", "future")
BEHAVIORS = ("migrate", "repeat-idempotency", "corrupt", "truncated", "backup-restore", "binary-rollback")


class MatrixError(ValueError):
    pass


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise MatrixError(f"cannot load {path}: {error}") from error


def fixture_path(matrix_path: Path, store: dict[str, Any], case: str) -> Path:
    return (matrix_path.parent / store["path"].replace("{case}", case)).resolve()


def inspect_fixture(path: Path, kind: str) -> tuple[int, str, bool]:
    raw = path.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if kind == "json":
        try:
            document = json.loads(raw)
        except json.JSONDecodeError as error:
            raise MatrixError(f"corrupt JSON fixture {path}: {error}") from error
        version = document.get("schemaVersion")
        preserved = document.get("payload", {}).get("preserved") is True
    elif kind == "sqlite":
        try:
            connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
            try:
                integrity = connection.execute("PRAGMA integrity_check").fetchone()
                version_row = connection.execute("SELECT value FROM metadata WHERE key='schema_version'").fetchone()
                payload_row = connection.execute("SELECT value FROM fixture_payload WHERE id='preserved'").fetchone()
            finally:
                connection.close()
        except sqlite3.Error as error:
            raise MatrixError(f"corrupt SQLite fixture {path}: {error}") from error
        if integrity != ("ok",):
            raise MatrixError(f"SQLite integrity failure for {path}: {integrity!r}")
        version = int(version_row[0]) if version_row else None
        preserved = payload_row == ("true",)
    else:
        raise MatrixError(f"unsupported fixture kind {kind!r}")
    if not isinstance(version, int) or not preserved:
        raise MatrixError(f"fixture {path} lacks its version or preservation sentinel")
    return version, digest, preserved


def evaluate(matrix_path: Path = DEFAULT_MATRIX) -> tuple[dict[str, Any], int]:
    matrix = load_json(matrix_path)
    if matrix.get("schemaVersion") != "jeden.migration-matrix.v1":
        raise MatrixError("unsupported matrix schemaVersion")
    if tuple(matrix.get("cases", {}).keys()) != CASES:
        raise MatrixError("matrix must declare N-2, N-1, N, future in order")
    if tuple(matrix.get("behavioralClassifications", [])) != BEHAVIORS:
        raise MatrixError("matrix behavioral classifications are incomplete")
    stores = matrix.get("stores")
    if not isinstance(stores, list) or len(stores) != 7:
        raise MatrixError("matrix must contain exactly seven persistence stores")

    fixture_results: list[dict[str, Any]] = []
    failures: list[str] = []
    for store in stores:
        expected_store = store.get("fixtureStore", store.get("runtimeStore"))
        for case in CASES:
            path = fixture_path(matrix_path, store, case)
            try:
                version, actual_digest, _ = inspect_fixture(path, store["kind"])
                expected_version = matrix["cases"][case]
                if version != expected_version:
                    raise MatrixError(f"{store['store']}/{case} has version {version}, expected {expected_version}")
                if actual_digest != store["sha256"].get(case):
                    raise MatrixError(f"{store['store']}/{case} digest changed: {actual_digest}")
                if store["kind"] == "json":
                    fixture_document = load_json(path)
                    if fixture_document.get("store") != expected_store or fixture_document.get("fixture") != case:
                        raise MatrixError(f"{store['store']}/{case} identity does not match its matrix row")
                fixture_results.append({"store": store["store"], "case": case, "classification": "Passed", "fixtureSha256": actual_digest})
            except (KeyError, OSError, MatrixError, ValueError) as error:
                failures.append(str(error))
                fixture_results.append({"store": store.get("store", "unknown"), "case": case, "classification": "Failed", "detail": str(error)})

    behavior_results = []
    for store in stores:
        prerequisite = f"executable migration adapter for runtime store {store['runtimeStore']}"
        for behavior in BEHAVIORS:
            behavior_results.append({
                "store": store["store"],
                "behavior": behavior,
                "classification": "NotRun",
                "prerequisites": [prerequisite],
            })

    if failures:
        classification, code = "Failed", 1
    else:
        classification, code = "NotRun", 3
    report = {
        "schemaVersion": "jeden.migration-matrix-report.v1",
        "mode": "fixture-contract",
        "matrixSha256": hashlib.sha256(matrix_path.read_bytes()).hexdigest(),
        "classification": classification,
        "compatibilityWindow": matrix["compatibilityWindow"],
        "fixtures": fixture_results,
        "behaviors": behavior_results,
    }
    if failures:
        report["failures"] = failures
    return report, code


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--matrix", type=Path, default=DEFAULT_MATRIX)
    args = parser.parse_args(argv)
    try:
        report, code = evaluate(args.matrix)
    except MatrixError as error:
        report, code = {"schemaVersion": "jeden.migration-matrix-report.v1", "classification": "Failed", "error": str(error)}, 1
    print(json.dumps(report, ensure_ascii=False, separators=(",", ":"), sort_keys=True))
    return code


if __name__ == "__main__":
    sys.exit(main())
