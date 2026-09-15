#!/usr/bin/env python3
"""Build and run the real helper from exported private crates while offline."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / ".wisent-output" / "release-tests" / str(uuid.uuid4())
REPORT.mkdir(parents=True)
trace = {"status": "failed", "commands": []}


def command(argv, env, expected=0):
    label = str(len(trace["commands"]))
    stdout_path = REPORT / (label + ".stdout")
    stderr_path = REPORT / (label + ".stderr")
    entry = {"argv": [str(value) for value in argv], "state": "running",
             "stdout_path": str(stdout_path), "stderr_path": str(stderr_path)}
    trace["commands"].append(entry)
    (REPORT / "report.json").write_text(json.dumps(trace, indent=2) + "\n")
    print("Running: " + " ".join(str(value) for value in argv), flush=True)
    print("Evidence: " + str(stderr_path), flush=True)
    with stdout_path.open("w") as stdout, stderr_path.open("w") as stderr:
        result = subprocess.run(argv, cwd=ROOT, env=env, text=True, stdout=stdout, stderr=stderr)
    result.stdout = stdout_path.read_text()
    result.stderr = stderr_path.read_text()
    entry.update(state="completed", exit_code=result.returncode,
                 stdout=result.stdout, stderr=result.stderr)
    (REPORT / "report.json").write_text(json.dumps(trace, indent=2) + "\n")
    if result.returncode != expected:
        raise AssertionError(f"expected exit {expected}, got {result.returncode}: {argv}\n{result.stderr}")
    return result


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def story():
    env = os.environ.copy()
    env.pop("WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR", None)
    env.update(GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="never")
    trace["source_revision"] = command(["git", "rev-parse", "HEAD"], env).stdout.strip()
    (REPORT / "source.patch").write_text(command(["git", "diff", "--binary", "HEAD"], env).stdout)
    lock_hash = sha256(ROOT / "Cargo.lock")
    wrapper = [sys.executable, "release/cargo.py", "cargo"]
    refused = command([*wrapper, "build", "--locked", "--offline"], env, expected=1)
    assert "WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR is required" in refused.stderr
    archive = REPORT / "private-cargo-sources.tar.gz"
    exported = command([sys.executable, "release/cargo.py", "export", str(archive)], env)
    receipt = json.loads(exported.stdout)
    assert sha256(archive) == receipt["sha256"]
    with tempfile.TemporaryDirectory(prefix="work-", dir=REPORT) as work:
        work = Path(work)
        inputs = work / "private sources"
        inputs.mkdir()
        # This is the archive produced above, not an arbitrary external tar file.
        with tarfile.open(archive) as tar:
            tar.extractall(inputs, filter="data")
        # Keep public registry paths stable so this test can reuse real compiler
        # output. Cargo's own package locations below prove the private sources
        # came from this archive rather than an ambient Git checkout.
        for key in tuple(env):
            if key.startswith(("GIT_", "GITHUB_", "GH_", "CARGO_REGISTRIES_")):
                env.pop(key)
        env.update(CARGO_NET_OFFLINE="true",
                   GIT_TERMINAL_PROMPT="0", GIT_CONFIG_NOSYSTEM="1",
                   GIT_CONFIG_GLOBAL=os.devnull,
                   WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR=str(inputs))
        metadata = json.loads(command([*wrapper, "metadata", "--locked", "--offline",
                                       "--format-version=1"], env).stdout)
        actual = {package["name"]: package for package in metadata["packages"]
                  if package["source"] and package["source"].startswith("git+")}
        expected = {package["name"]: package for package in receipt["provenance"]["packages"]}
        assert set(actual) == set(expected)
        for name, package in actual.items():
            assert package["source"] == expected[name]["source"]
            assert inputs / "sources" in Path(package["manifest_path"]).parents, package
        # The ordinary product target cache avoids copying a checkout or changing
        # the installed binary; only the release helper is built and executed.
        command([*wrapper, "build", "--locked", "--offline", "--release",
                 "--bin", "jeden-sandbox-helper"], env)
        helper = ROOT / "target" / "release" / "jeden-sandbox-helper"
        version = command([str(helper), "--version"], env).stdout.strip()
        assert version.startswith("jeden-sandbox-helper "), version
        trace["artifact"] = {"path": str(helper), "sha256": sha256(helper), "version": version}
        assert sha256(ROOT / "Cargo.lock") == lock_hash
        provenance_path = inputs / "provenance.json"
        provenance = json.loads(provenance_path.read_text())
        original = provenance_path.read_bytes()
        provenance["cargo_lock_sha256"] = hashlib.sha256(b"different lockfile").hexdigest()
        provenance_path.write_text(json.dumps(provenance))
        mismatch = command([*wrapper, "build", "--locked", "--offline"], env, expected=1)
        assert "private Cargo sources do not match Cargo.lock" in mismatch.stderr
        provenance_path.write_bytes(original)
        package = receipt["provenance"]["packages"][0]
        missing = inputs / "sources" / (package["name"] + "-" + package["version"])
        shutil.rmtree(missing)
        command([*wrapper, "build", "--locked", "--offline", "--release",
                 "--bin", "jeden-sandbox-helper"], env, expected=101)
        assert sha256(ROOT / "Cargo.lock") == lock_hash
    trace["status"] = "passed"


try:
    story()
except Exception as error:
    trace["error"] = str(error)
    print(str(error), file=sys.stderr)
finally:
    (REPORT / "report.json").write_text(json.dumps(trace, indent=2) + "\n")
    print(str(REPORT / "report.json"))
sys.exit(0 if trace["status"] == "passed" else 1)
