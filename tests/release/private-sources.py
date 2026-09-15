#!/usr/bin/env python3
"""Build and run the real helper from exported private crates while offline."""
import argparse
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


def stage_native(env, binary):
    stager = [sys.executable, "release/cargo.py", "stage", "--bin", binary]
    no_output = env.copy()
    no_output.pop("WISENT_OUTPUT_DIR", None)
    refused = command(stager, no_output, expected=1)
    assert "WISENT_OUTPUT_DIR is required for native staging" in refused.stderr
    with tempfile.TemporaryDirectory(prefix="stage-", dir=REPORT) as work:
        work = Path(work)
        staged = work / "staged native"
        worker_env = {**env, "PATH": os.defpath, "WISENT_OUTPUT_DIR": str(staged)}
        trace["worker_path"] = worker_env["PATH"]
        refused = command(stager, {**worker_env, "WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR": ""}, expected=1)
        assert "WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR is required" in refused.stderr
        assert not staged.exists()
        missing_toolchain = {**worker_env, "PATH": "", "CARGO_HOME": str(work / "missing-cargo-home")}
        refused = command(stager, missing_toolchain, expected=1)
        assert "Cargo is unavailable on PATH and at " in refused.stderr
        assert not staged.exists()
        command(stager, worker_env)
        executable = staged / "bin" / binary
        version = command([str(executable), "--version"], env).stdout.strip()
        assert sha256(executable) == sha256(ROOT / "target" / "release" / binary)
        trace["artifact"] = {"path": str(executable), "sha256": sha256(executable), "version": version}


def story():
    env = os.environ.copy()
    env.pop("WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR", None)
    env.pop("WISENT_OUTPUT_DIR", None)
    env.update(GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="never")
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
        # Release workers may have only the system PATH. Cargo must still be
        # found in the configured Cargo home, without shell startup files.
        worker_env = {**env, "PATH": os.defpath}
        trace["worker_path"] = worker_env["PATH"]
        metadata = json.loads(command([*wrapper, "metadata", "--locked", "--offline",
                                       "--format-version=1"], worker_env).stdout)
        actual = {package["name"]: package for package in metadata["packages"]
                  if package["source"] and package["source"].startswith("git+")}
        expected = {package["name"]: package for package in receipt["provenance"]["packages"]}
        assert set(actual) == set(expected)
        for name, package in actual.items():
            assert package["source"] == expected[name]["source"]
            assert inputs / "sources" in Path(package["manifest_path"]).parents, package
        stage_native(env, "jeden-sandbox-helper")
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", action="store_true",
                        help="qualify native CLI staging from the release worker's declared input")
    args = parser.parse_args()
    env = os.environ.copy()
    revision = env.get("WISENT_SOURCE_COMMIT")
    trace["source_revision"] = revision or command(["git", "rev-parse", "HEAD"], env).stdout.strip()
    if not revision:
        (REPORT / "source.patch").write_text(command(["git", "diff", "--binary", "HEAD"], env).stdout)
    if args.stage:
        trace["scope"] = "native staging from declared input"
        env["CARGO_NET_OFFLINE"] = "true"
        lock_hash = sha256(ROOT / "Cargo.lock")
        stage_native(env, "jeden")
        assert sha256(ROOT / "Cargo.lock") == lock_hash
        trace["status"] = "passed"
    else:
        trace["scope"] = "private source export and native helper staging"
        story()
except Exception as error:
    trace["error"] = str(error)
    print(str(error), file=sys.stderr)
finally:
    (REPORT / "report.json").write_text(json.dumps(trace, indent=2) + "\n")
    print(str(REPORT / "report.json"))
sys.exit(0 if trace["status"] == "passed" else 1)
