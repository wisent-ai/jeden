#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import stat
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release.yml"
PROMOTE_WORKFLOW = ROOT / ".github/workflows/promote.yml"
SOURCE_SHA_EXPRESSION = "${{ github.event.workflow_run.head_sha || github.sha }}"
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


class ReleaseWorkflowContractTests(unittest.TestCase):
    def test_automatic_canary_is_bound_to_the_successful_contractual_ci_revision(self) -> None:
        release = load_workflow(RELEASE_WORKFLOW)
        promotion = load_workflow(PROMOTE_WORKFLOW)
        entrypoints = release["on"]
        jobs = release["jobs"]
        job = jobs["build-sign-publish"]

        self.assertEqual({"build-sign-publish"}, set(jobs))
        self.assertEqual({"workflow_run", "push", "workflow_dispatch"}, set(entrypoints))
        self.assertEqual(
            {"workflows": ["contractual-ci"], "types": ["completed"], "branches": ["main"]},
            entrypoints["workflow_run"],
        )
        self.assertEqual({"tags": ["v[0-9]+.[0-9]+.[0-9]+*"]}, entrypoints["push"])
        self.assertEqual(
            "github.event_name != 'workflow_run' || "
            "(github.event.workflow_run.conclusion == 'success' && "
            "github.event.workflow_run.event == 'push' && "
            "github.event.workflow_run.head_branch == 'main' && "
            "github.event.workflow_run.head_repository.full_name == github.repository)",
            " ".join(job["if"].split()),
        )
        self.assertEqual(SOURCE_SHA_EXPRESSION, job["env"]["SOURCE_SHA"])
        checkout = step_named(job, "Check out the exact source revision")
        self.assertEqual(SOURCE_SHA_EXPRESSION, checkout["with"]["ref"])

        version = step_named(job, "Resolve and validate release version")
        crate_version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
        major, minor, patch = (int(part) for part in crate_version.split("."))
        source_sha = "abcdef0123456789abcdef0123456789abcdef01"
        cases = (
            (
                {"EVENT_NAME": "workflow_run", "INPUT_VERSION": "", "GITHUB_REF": "", "GITHUB_REF_NAME": ""},
                f"{major}.{minor}.{patch + 1}-canary.42.3.sha{source_sha[:12]}",
            ),
            (
                {"EVENT_NAME": "workflow_dispatch", "INPUT_VERSION": "2.3.4", "GITHUB_REF": "", "GITHUB_REF_NAME": ""},
                "2.3.4",
            ),
            (
                {"EVENT_NAME": "push", "INPUT_VERSION": "", "GITHUB_REF": "refs/tags/v3.2.1", "GITHUB_REF_NAME": "v3.2.1"},
                "3.2.1",
            ),
        )
        for environment, expected_version in cases:
            with self.subTest(event=environment["EVENT_NAME"]), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / "github-output"
                execute_step(
                    version,
                    ROOT,
                    environment
                    | {
                        "SOURCE_SHA": source_sha,
                        "GITHUB_RUN_NUMBER": "42",
                        "GITHUB_RUN_ATTEMPT": "3",
                        "GITHUB_OUTPUT": str(output),
                    },
                )
                self.assertEqual(f"version={expected_version}\n", output.read_text(encoding="utf-8"))

        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            (work / "dist").mkdir()
            provenance = step_named(job, "Generate provenance statement hook")
            execute_step(
                provenance,
                work,
                {
                    "ARTIFACT_NAME": "jeden.tar.gz",
                    "ARTIFACT_SHA256": "1" * 64,
                    "ARTIFACT_SIZE": "1",
                    "TARGET_TRIPLE": "test-target",
                    "SOURCE_SHA": source_sha,
                    "GITHUB_SERVER_URL": "https://github.example",
                    "GITHUB_REPOSITORY": "owner/repository",
                    "GITHUB_RUN_ID": "42",
                    "GITHUB_RUN_ATTEMPT": "3",
                },
            )
            statement = json.loads((work / "dist/provenance.intoto.json").read_text(encoding="utf-8"))
            dependencies = statement["predicate"]["buildDefinition"]["resolvedDependencies"]
            self.assertEqual([{"uri": f"git+https://github.example/owner/repository@{source_sha}"}], dependencies)

            (work / "dist/manifest.dsse.json").write_text("{}\n", encoding="utf-8")
            binary_directory = work / "bin"
            binary_directory.mkdir()
            curl = binary_directory / "curl"
            curl.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            curl.chmod(curl.stat().st_mode | stat.S_IXUSR)
            gate_output = work / "gate-output"
            gate = step_named(job, "Publish immutable DSSE envelope and gate digest record")
            execute_step(
                gate,
                work,
                {
                    "PATH": f"{binary_directory}:{os.environ['PATH']}",
                    "SOURCE_SHA": source_sha,
                    "ARTIFACT_SHA256": "1" * 64,
                    "SBOM_SHA256": "2" * 64,
                    "PROVENANCE_SHA256": "3" * 64,
                    "PAYLOAD_SHA256": "4" * 64,
                    "RELEASE_STORE_BASE_URL": "https://release.example",
                    "RELEASE_ACCESS_TOKEN": "test-token",
                    "GITHUB_RUN_ID": "42",
                    "GITHUB_OUTPUT": str(gate_output),
                },
            )
            gate_record = json.loads((work / "dist/release-gate-digests.json").read_text(encoding="utf-8"))
            self.assertEqual(source_sha, gate_record["sourceSha"])

        self.assertEqual({"workflow_dispatch"}, set(promotion["on"]))


if __name__ == "__main__":
    unittest.main()
