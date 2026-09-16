#!/usr/bin/env python3
"""Verify persisted diagnostics against a real unavailable startup dependency."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import uuid

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--model", required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = ROOT / "target/startup-diagnostics" / str(uuid.uuid4())
    workspace = report / "workspace"
    workspace.mkdir(parents=True)
    sessions = report / "sessions"
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    (report / "source.patch").write_bytes(subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=ROOT))
    argv = [str(binary), "run", "Read the current directory without changing files.",
            "--cwd", str(workspace), "--model", args.model]
    evidence = {"source_revision": revision, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "command": argv, "status": "failed"}
    print("Evidence: " + str(report), flush=True)
    try:
        with (report / "stdout").open("w") as stdout, (report / "stderr").open("w") as stderr:
            result = subprocess.run(argv, cwd=ROOT, stdout=stdout, stderr=stderr,
                                    env={**os.environ, "JEDEN_SESSION_ROOT": str(sessions)})
        evidence["exit_code"] = result.returncode
        assert result.returncode != 0, "Refusal test requires a real unavailable startup dependency"
        events = [json.loads(line) for path in sessions.glob("*/transcript.jsonl")
                  for line in path.read_text().splitlines() if line]
        failures = [event for event in events if event["payload"]["type"] == "run_error"]
        assert failures, "Startup refusal was not retained"
        failure = failures[-1]["payload"]["data"]
        assert failure["operation"] == "prepare_turn", failure
        assert failure["message"] in (report / "stderr").read_text(), failure
        assert any(event["payload"]["type"] == "completion_state"
                   and not event["payload"]["data"]["complete"] for event in events)
        evidence.update(status="passed", observed_failure=failure)
    finally:
        (report / "report.json").write_text(json.dumps(evidence, indent=2) + "\n")


if __name__ == "__main__":
    main()
