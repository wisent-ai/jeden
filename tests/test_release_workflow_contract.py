#!/usr/bin/env python3
from __future__ import annotations

import datetime
import hashlib
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any, Iterator

ROOT = Path(__file__).resolve().parents[1]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release.yml"
HEAD_SHA = "${{ github.event.workflow_run.head_sha }}"
RUBY_YAML_TO_JSON = """
require 'json'
require 'yaml'
document = YAML.safe_load(File.read(ARGV.fetch(0)), permitted_classes: [], permitted_symbols: [], aliases: false)
puts JSON.generate(document)
"""


def load_workflow(path: Path) -> dict[str, Any]:
    completed = subprocess.run(
        ["ruby", "-e", RUBY_YAML_TO_JSON, str(path)],
        check=True,
        capture_output=True,
        text=True,
    )
    document = json.loads(completed.stdout)
    # Psych follows YAML 1.1 and decodes the unquoted mapping key `on` as true.
    document["on"] = document.pop("true")
    return document


def step_named(job: dict[str, Any], name: str) -> dict[str, Any]:
    return next(step for step in job["steps"] if step.get("name") == name)


def execute_step(step: dict[str, Any], cwd: Path, environment: dict[str, str]) -> None:
    subprocess.run(
        ["bash", "-c", step["run"]],
        cwd=cwd,
        env=os.environ | environment,
        check=True,
        capture_output=True,
        text=True,
    )


def scalar_strings(value: Any) -> Iterator[str]:
    if isinstance(value, dict):
        for key, child in value.items():
            yield str(key)
            yield from scalar_strings(child)
    elif isinstance(value, list):
        for child in value:
            yield from scalar_strings(child)
    elif isinstance(value, str):
        yield value


class ReleaseWorkflowContractTests(unittest.TestCase):
    def test_github_only_builds_unsigned_handoffs_from_contractual_ci(self) -> None:
        release = load_workflow(RELEASE_WORKFLOW)

        self.assertEqual(
            {
                "workflow_run": {
                    "workflows": ["contractual-ci"],
                    "types": ["completed"],
                    "branches": ["main"],
                }
            },
            release["on"],
        )
        self.assertEqual({"contents": "read"}, release["permissions"])
        self.assertEqual({"build-evidence"}, set(release["jobs"]))

        job = release["jobs"]["build-evidence"]
        self.assertEqual(
            "github.event.workflow_run.conclusion == 'success' && "
            "github.event.workflow_run.event == 'push' && "
            "github.event.workflow_run.head_branch == 'main' && "
            "github.event.workflow_run.head_repository.full_name == github.repository",
            " ".join(job["if"].split()),
        )
        self.assertEqual(HEAD_SHA, job["env"]["SOURCE_SHA"])
        checkout = step_named(job, "Check out the exact contractual CI revision")
        self.assertEqual(HEAD_SHA, checkout["with"]["ref"])
        self.assertIs(checkout["with"]["persist-credentials"], False)

        self.assertEqual(
            [
                {
                    "target": "x86_64-unknown-linux-gnu",
                    "runner": "ubuntu-24.04",
                    "executable": "jeden",
                },
                {
                    "target": "aarch64-apple-darwin",
                    "runner": "macos-14",
                    "executable": "jeden",
                },
                {
                    "target": "x86_64-pc-windows-msvc",
                    "runner": "windows-2022",
                    "executable": "jeden.exe",
                },
            ],
            job["strategy"]["matrix"]["include"],
        )

        self.assertNotIn("environment", job)
        self.assertEqual(
            {
                "actions/checkout@v4",
                "dtolnay/rust-toolchain@stable",
                "anchore/sbom-action@v0.20.6",
                "actions/upload-artifact@v4",
            },
            {step["uses"] for step in job["steps"] if "uses" in step},
        )

        upload = step_named(job, "Upload unsigned build evidence for the publisher")
        self.assertEqual("actions/upload-artifact@v4", upload["uses"])
        self.assertEqual(
            (
                "dist/${{ steps.artifact.outputs.name }}",
                "dist/build-handoff.json",
                "dist/sbom.spdx.json",
                "dist/provenance.intoto.json",
            ),
            tuple(upload["with"]["path"].splitlines()),
        )
        self.assertEqual("error", upload["with"]["if-no-files-found"])
        retention_days = upload["with"]["retention-days"]
        self.assertIsInstance(retention_days, int)
        self.assertGreaterEqual(retention_days, 1)
        self.assertLessEqual(retention_days, 7)

        policy_text = "\n".join(scalar_strings(release)).lower()
        for forbidden_capability in (
            "${{ secrets.",
            "id-token",
            "oidc",
            "kms",
            "signature",
            "signing",
            "dsse",
            "release_store",
            "release-store",
            "release store",
            "channel",
            "gh release",
            "actions/create-release",
            "softprops/action-gh-release",
        ):
            with self.subTest(forbidden_capability=forbidden_capability):
                self.assertNotIn(forbidden_capability, policy_text)

    def test_handoff_is_canonical_strict_and_binds_all_unsigned_evidence(self) -> None:
        release = load_workflow(RELEASE_WORKFLOW)
        handoff_step = step_named(release["jobs"]["build-evidence"], "Create publisher build handoff")
        source_sha = "abcdef0123456789abcdef0123456789abcdef01"
        artifact_name = "jeden-0.2.0-canary.42.3.shaabcdef012345-test-target.tar.gz"
        artifact_bytes = b"immutable executable archive"
        sbom_bytes = b'{"spdxVersion":"SPDX-2.3"}\n'
        provenance_bytes = b'{"_type":"https://in-toto.io/Statement/v1"}\n'

        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            dist = work / "dist"
            dist.mkdir()
            (dist / artifact_name).write_bytes(artifact_bytes)
            (dist / "sbom.spdx.json").write_bytes(sbom_bytes)
            (dist / "provenance.intoto.json").write_bytes(provenance_bytes)
            execute_step(
                handoff_step,
                work,
                {
                    "GITHUB_REPOSITORY": "owner/repository",
                    "SOURCE_SHA": source_sha,
                    "BUILD_VERSION": "0.2.0-canary.42.3.shaabcdef012345",
                    "MINIMUM_VERSION": "0.1.0",
                    "CONTRACTUAL_CI_RUN_ID": "1001",
                    "CONTRACTUAL_CI_RUN_ATTEMPT": "2",
                    "GITHUB_RUN_ID": "2002",
                    "GITHUB_RUN_ATTEMPT": "3",
                    "TARGET_TRIPLE": "test-target",
                    "ARTIFACT_NAME": artifact_name,
                    "ARTIFACT_SHA256": hashlib.sha256(artifact_bytes).hexdigest(),
                    "ARTIFACT_SIZE": str(len(artifact_bytes)),
                },
            )

            encoded = (dist / "build-handoff.json").read_text(encoding="utf-8")
            handoff = json.loads(encoded)
            self.assertEqual(
                {
                    "schema",
                    "repository",
                    "headSha",
                    "version",
                    "minimumVersion",
                    "createdAt",
                    "contractualCiRunId",
                    "contractualCiRunAttempt",
                    "buildRunId",
                    "buildRunAttempt",
                    "targetTriple",
                    "artifact",
                    "sbom",
                    "provenance",
                },
                set(handoff),
            )
            self.assertEqual({"name", "sha256", "size"}, set(handoff["artifact"]))
            self.assertEqual({"name", "sha256"}, set(handoff["sbom"]))
            self.assertEqual({"name", "sha256"}, set(handoff["provenance"]))
            self.assertEqual("jeden.release-build-handoff/v1", handoff["schema"])
            self.assertEqual("owner/repository", handoff["repository"])
            self.assertEqual(source_sha, handoff["headSha"])
            self.assertEqual("0.2.0-canary.42.3.shaabcdef012345", handoff["version"])
            self.assertEqual("0.1.0", handoff["minimumVersion"])
            self.assertEqual("1001", handoff["contractualCiRunId"])
            self.assertEqual(2, handoff["contractualCiRunAttempt"])
            self.assertEqual("2002", handoff["buildRunId"])
            self.assertEqual(3, handoff["buildRunAttempt"])
            self.assertEqual("test-target", handoff["targetTriple"])
            self.assertEqual(
                {
                    "name": artifact_name,
                    "sha256": hashlib.sha256(artifact_bytes).hexdigest(),
                    "size": len(artifact_bytes),
                },
                handoff["artifact"],
            )
            self.assertEqual(
                {"name": "sbom.spdx.json", "sha256": hashlib.sha256(sbom_bytes).hexdigest()},
                handoff["sbom"],
            )
            self.assertEqual(
                {
                    "name": "provenance.intoto.json",
                    "sha256": hashlib.sha256(provenance_bytes).hexdigest(),
                },
                handoff["provenance"],
            )
            self.assertRegex(handoff["createdAt"], r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
            created_at = datetime.datetime.fromisoformat(handoff["createdAt"].replace("Z", "+00:00"))
            self.assertEqual(datetime.timezone.utc, created_at.tzinfo)
            self.assertEqual(
                json.dumps(handoff, sort_keys=True, separators=(",", ":")) + "\n",
                encoded,
            )


if __name__ == "__main__":
    unittest.main()
