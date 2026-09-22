"""Real CLI admission and durable-refusal journeys; no model stand-ins.

Run: python3 -m unittest discover -s tests/pursuit -p test_requests.py -v
Fleet qualification supplies JEDEN_TEST_BINARY and WISENT_SOURCE_COMMIT for the
built candidate. Missing source binding fails qualification.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import unittest
from uuid import uuid4

ROOT = Path(__file__).resolve().parents[2]


class DurableRequests(unittest.TestCase):
    def setUp(self):
        self.root = ROOT / "target/pursuit-runs" / uuid4().hex
        self.root.mkdir(parents=True, mode=0o700)
        self.report = {"journey": self.id(), "state": "failed", "commands": []}
        self.addCleanup(self.retain)
        self.binary = shutil.which(os.environ.get("JEDEN_TEST_BINARY", "jeden"))
        self.assertIsNotNone(self.binary, "The real Jeden candidate is unavailable")
        self.binary = str(Path(self.binary).resolve())
        self.report["binary"] = self.binary
        with open(self.binary, "rb") as executable:
            self.report["binary_sha256"] = hashlib.file_digest(executable, "sha256").hexdigest()
        self.report["candidate_source_revision"] = os.environ.get("WISENT_SOURCE_COMMIT")
        revision = self.command(["git", "rev-parse", "HEAD"], env=os.environ)
        self.assertEqual(revision.returncode, os.EX_OK, revision.stderr)
        self.report["checkout_revision"] = revision.stdout.strip()
        self.env = {**os.environ, "HOME": str(self.root / "home"),
                    "JEDEN_SESSION_ROOT": str(self.root / "sessions"),
                    "JEDEN_PURSUIT_STATE_ROOT": str(self.root / "requests")}
        self.env.pop("JEDEN_LANGUAGE", None)
        Path(self.env["HOME"]).mkdir(mode=0o700)
        self.cli("--version")
        self.identity = "request-" + uuid4().hex
        # A real missing checkout prevents external work even if a regression
        # admits the authority request that the first case expects to refuse.
        self.request = {
            "schema_version": 1, "request_id": self.identity,
            "initiative_id": self.identity,
            "objective": "Inspect the declared repository without modifying it.",
            "cwd": str(ROOT.resolve()), "evidence_refs": [str(Path(__file__).resolve())],
            "budget_usd": "1", "allow_write": False, "allow_command": False,
            "repositories": ["wisent-ai/unavailable-" + uuid4().hex],
        }
        self.document = self.root / "request.json"

    def retain(self):
        (self.root / "report.json").write_text(json.dumps(self.report, indent=2) + "\n")
        print(f"Pursuit journey evidence: {self.root}")

    def command(self, argv, *, env):
        result = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, text=True)
        self.report["commands"].append({"argv": argv, "cwd": str(ROOT),
                                       "exit_status": result.returncode,
                                       "stdout": result.stdout, "stderr": result.stderr})
        return result

    def cli(self, *args):
        return self.command([self.binary, *args], env=self.env)

    def submit(self):
        self.document.write_text(json.dumps(self.request) + "\n")
        return self.cli("pursue", "--request-file", str(self.document), "--json")

    def response(self, output):
        self.assertEqual(output.returncode, os.EX_OK, output.stdout + output.stderr)
        return json.loads(output.stdout)

    def saved(self):
        path = self.root / "requests" / self.identity / "state.sqlite3"
        with sqlite3.connect(path.as_uri() + "?mode=ro", uri=True) as connection:
            return {key: json.loads(data) for key, data in connection.execute(
                "SELECT key, data FROM values_store ORDER BY key")}

    def passed(self):
        # The build producer supplies provenance; the checkout cannot identify
        # the source of a different executable found on PATH.
        self.assertEqual(self.report["candidate_source_revision"],
                         self.report["checkout_revision"],
                         "Candidate source binding is absent or differs from this journey revision")
        self.report["state"] = "passed"

    def test_authority_refusal_does_not_admit_request(self):
        self.request["allow_write"] = True
        refused = self.submit()
        self.assertNotEqual(refused.returncode, os.EX_OK)
        self.assertIn("authority_required:", refused.stderr)
        self.assertFalse((self.root / "requests" / self.identity).exists(),
                         "An unauthorized request must not acquire durable execution state")
        unknown = self.cli("pursue", "--status", self.identity, "--json")
        self.assertNotEqual(unknown.returncode, os.EX_OK)
        self.assertFalse((self.root / "requests" / self.identity).exists())
        self.passed()

    def test_blocked_request_survives_reopen_and_rejects_rebinding(self):
        blocked = self.response(self.submit())
        self.assertEqual(blocked["state"], "blocked", blocked)
        self.assertIn(self.request["repositories"][0], blocked["error"])
        before = self.saved()
        self.assertEqual(before["request"], self.request)
        self.assertEqual(before["response"]["state"], "blocked")
        repeated = self.response(self.submit())
        self.assertEqual(repeated, blocked)
        self.assertEqual(self.saved(), before)
        retained = self.response(self.cli("pursue", "--status", self.identity, "--json"))
        self.assertEqual(retained, blocked)
        self.request["objective"] = "Different work must not replace the retained request."
        conflict = self.submit()
        self.assertNotEqual(conflict.returncode, os.EX_OK)
        self.assertIn("request_id_conflict:", conflict.stderr)
        self.assertEqual(self.saved(), before)
        resumed = self.response(self.cli("pursue", "--resume-run", self.identity, "--json"))
        self.assertEqual(resumed["state"], "blocked")
        self.assertEqual(self.saved()["request"], before["request"])
        self.assertFalse((self.root / "requests" / self.identity / "inference.sqlite3").exists(),
                         "A missing declared checkout must refuse before inference")
        self.report["persisted_request"] = self.saved()["request"]
        self.passed()


if __name__ == "__main__":
    unittest.main()
