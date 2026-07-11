#!/usr/bin/env python3
"""Fail-closed adapter for versioned conformance evidence fixtures."""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
CONTRACTS = HERE / "contracts"
PROTOCOL_VERSION = "jeden.behavior-check.v2"
REQUIRED = (
    "protocolVersion",
    "checkVersion",
    "fixtureDigest",
    "commandOrScenarioId",
    "startedAt",
    "finishedAt",
    "expiresAt",
    "attempts",
    "outcome",
)
OUTCOMES = frozenset({"passed", "failed", "external-blocked"})


class EvidenceError(ValueError):
    pass


def load(path: Path) -> dict[str, Any]:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise EvidenceError(f"cannot load {path}: {error}") from error
    if not isinstance(document, dict):
        raise EvidenceError("evidence root must be an object")
    return document


def classify(path: Path, now_ms: int | None = None) -> dict[str, Any]:
    document = load(path)
    missing = [key for key in REQUIRED if key not in document]
    reasons: list[str] = []
    if missing:
        reasons.append(f"missing required fields: {', '.join(missing)}")
    if document.get("protocolVersion") != PROTOCOL_VERSION or document.get("checkVersion") != 2:
        reasons.append("unsupported behavior-check protocol or check version")
    digest = document.get("fixtureDigest")
    if not isinstance(digest, str) or len(digest) != 64 or any(character not in "0123456789abcdefABCDEF" for character in digest):
        reasons.append("fixtureDigest must be 64 hexadecimal characters")
    started = document.get("startedAt")
    finished = document.get("finishedAt")
    expires = document.get("expiresAt")
    if not all(isinstance(value, int) and value >= 0 for value in (started, finished, expires)):
        reasons.append("evidence timestamps must be non-negative integers")
    elif started > finished or finished > expires:
        reasons.append("evidence timestamps are inconsistent")
    current = int(time.time() * 1000) if now_ms is None else now_ms
    if isinstance(expires, int) and expires < current:
        reasons.append(f"behavioral evidence expired at {expires}")
    attempts = document.get("attempts")
    if not isinstance(attempts, list) or not attempts:
        reasons.append("at least one execution attempt is required")
    else:
        expected_attempt = 1
        for attempt in attempts:
            if not isinstance(attempt, dict):
                reasons.append("execution attempt must be an object")
                continue
            if attempt.get("attempt") != expected_attempt:
                reasons.append("attempt numbers must be contiguous from one")
            expected_attempt += 1
            attempt_started = attempt.get("startedAt")
            attempt_finished = attempt.get("finishedAt")
            if not isinstance(attempt_started, int) or not isinstance(attempt_finished, int) or attempt_started > attempt_finished:
                reasons.append("execution attempt timestamps are inconsistent")
            if attempt.get("outcome") != "passed":
                reasons.append(f"execution attempt outcome is {attempt.get('outcome')!r}")
    outcome = document.get("outcome")
    if outcome not in OUTCOMES:
        reasons.append("evidence outcome is unsupported")
    elif outcome != "passed":
        reasons.append(f"behavioral scenario outcome is {outcome}")
    classification = "Passed" if not reasons else "Failed"
    return {
        "schemaVersion": "jeden.behavioral-gate-report.v1",
        "evidence": path.name,
        "classification": classification,
        "reasons": reasons,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("evidence", nargs="+", type=Path)
    parser.add_argument("--now-ms", type=int)
    parser.add_argument("--expect-rejected", action="store_true")
    arguments = parser.parse_args(argv)
    reports = [classify(path, arguments.now_ms) for path in arguments.evidence]
    print(json.dumps({"schemaVersion": "jeden.behavioral-gates-report.v1", "reports": reports}, sort_keys=True, separators=(",", ":")))
    rejected = all(report["classification"] == "Failed" for report in reports)
    if arguments.expect_rejected:
        return 0 if rejected else 1
    return 0 if all(report["classification"] == "Passed" for report in reports) else 1


if __name__ == "__main__":
    sys.exit(main())
