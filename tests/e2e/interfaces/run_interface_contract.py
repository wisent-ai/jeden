#!/usr/bin/env python3
"""Compare adapter captures by normalized semantic ledger, without invoking runtime adapters."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

ADAPTERS = ("tui", "line", "json", "rust", "typescript", "python", "acp", "headless")
VOLATILE_KEYS = frozenset({"adapter", "eventId", "requestId", "sequence", "sessionId", "streamId", "timestamp", "traceId", "transport"})
FIXTURES = Path(__file__).with_name("fixtures")
DEFAULT_SCENARIO = FIXTURES / "representative-scenario-v1.json"
DEFAULT_CAPTURES = FIXTURES / "fixture-contract-captures-v1.json"


class ContractError(ValueError):
    pass


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ContractError(f"cannot load {path}: {error}") from error


def canonical(value: Any) -> Any:
    if isinstance(value, dict):
        return {key: canonical(item) for key, item in sorted(value.items()) if key not in VOLATILE_KEYS}
    if isinstance(value, list):
        return [canonical(item) for item in value]
    return value


def digest(value: Any) -> str:
    encoded = json.dumps(canonical(value), ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode()
    return hashlib.sha256(encoded).hexdigest()


def load_captures(path: Path) -> dict[str, Any]:
    document = load_json(path)
    if document.get("schemaVersion") == "jeden.interface-capture-overlay.v1":
        base_path = path.parent / document.get("base", "")
        base = copy.deepcopy(load_captures(base_path))
        replacement = document.get("replace", {})
        adapter = replacement.get("adapter")
        index = replacement.get("eventIndex")
        matches = [capture for capture in base["captures"] if capture.get("adapter") == adapter]
        if len(matches) != 1 or not isinstance(index, int):
            raise ContractError("overlay must identify one adapter event")
        try:
            matches[0]["events"][index] = replacement["event"]
        except (IndexError, KeyError, TypeError) as error:
            raise ContractError("overlay replacement is outside the capture") from error
        return base
    if document.get("schemaVersion") != "jeden.interface-captures.v1":
        raise ContractError("unsupported capture schemaVersion")
    return document


def evaluate(scenario_path: Path, captures_path: Path) -> tuple[dict[str, Any], int]:
    scenario = load_json(scenario_path)
    captures = load_captures(captures_path)
    if scenario.get("schemaVersion") != "jeden.interface-scenario.v1":
        raise ContractError("unsupported scenario schemaVersion")
    if captures.get("scenarioId") != scenario.get("scenarioId"):
        raise ContractError("capture scenarioId does not match scenario")
    expected = canonical(scenario.get("expectedSemanticLedger"))
    if not isinstance(expected, list) or not expected:
        raise ContractError("scenario requires a non-empty expectedSemanticLedger")

    by_adapter: dict[str, dict[str, Any]] = {}
    for capture in captures.get("captures", []):
        adapter = capture.get("adapter")
        if adapter not in ADAPTERS or adapter in by_adapter:
            raise ContractError(f"unknown or duplicate adapter {adapter!r}")
        by_adapter[adapter] = capture

    results: list[dict[str, Any]] = []
    failed = False
    blocked = False
    expected_digest = digest(expected)
    for adapter in ADAPTERS:
        capture = by_adapter.get(adapter)
        if capture is None:
            results.append({"adapter": adapter, "classification": "ExternalBlocked", "prerequisites": [f"{adapter} capture artifact"]})
            blocked = True
            continue
        status = capture.get("status")
        if status == "external-blocked":
            prerequisites = capture.get("prerequisites")
            if not isinstance(prerequisites, list) or not prerequisites or not all(isinstance(item, str) and item for item in prerequisites):
                raise ContractError(f"{adapter} ExternalBlocked capture requires concrete prerequisites")
            results.append({"adapter": adapter, "classification": "ExternalBlocked", "prerequisites": sorted(prerequisites)})
            blocked = True
            continue
        if status != "captured" or not isinstance(capture.get("events"), list):
            raise ContractError(f"{adapter} capture must be captured or external-blocked")
        actual = canonical(capture["events"])
        actual_digest = digest(actual)
        if actual == expected:
            results.append({"adapter": adapter, "classification": "Passed", "semanticLedgerSha256": actual_digest})
        else:
            results.append({"adapter": adapter, "classification": "Failed", "expectedSha256": expected_digest, "actualSha256": actual_digest})
            failed = True

    if failed:
        classification, code = "Failed", 1
    elif blocked:
        classification, code = "ExternalBlocked", 2
    else:
        classification, code = "Passed", 0
    report = {
        "schemaVersion": "jeden.interface-equivalence-report.v1",
        "scenarioId": scenario["scenarioId"],
        "mode": "fixture-contract" if captures.get("evidenceClass") == "FixtureContract" else "captured-results",
        "fixtureSha256": hashlib.sha256(scenario_path.read_bytes()).hexdigest(),
        "expectedSemanticLedgerSha256": expected_digest,
        "classification": classification,
        "adapters": results,
    }
    return report, code


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--scenario", type=Path, default=DEFAULT_SCENARIO)
    parser.add_argument("--captures", type=Path, default=DEFAULT_CAPTURES)
    args = parser.parse_args(argv)
    try:
        report, code = evaluate(args.scenario, args.captures)
    except ContractError as error:
        report, code = {"schemaVersion": "jeden.interface-equivalence-report.v1", "classification": "Failed", "error": str(error)}, 1
    print(json.dumps(report, ensure_ascii=False, separators=(",", ":"), sort_keys=True))
    return code


if __name__ == "__main__":
    sys.exit(main())
