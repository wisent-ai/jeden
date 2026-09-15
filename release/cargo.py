#!/usr/bin/env python3
"""Export locked private Cargo sources and consume them in Stado release jobs."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from urllib.parse import parse_qs, urlsplit, urlunsplit

ROOT = Path(__file__).resolve().parent.parent
INPUT_ENV = "WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR"


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def run(argv, capture=False):
    return subprocess.run(argv, cwd=ROOT, check=True, text=True,
                          stdout=subprocess.PIPE if capture else None)


def source_config(sources):
    lines = []
    for index, source in enumerate(sorted(sources)):
        parsed = urlsplit(source.removeprefix("git+"))
        options = parse_qs(parsed.query, strict_parsing=True)
        if set(options) - {"rev", "tag", "branch"}:
            raise ValueError("unsupported locked Git source: " + source)
        url = urlunsplit((parsed.scheme, parsed.netloc, parsed.path, "", ""))
        lines.extend([f"[source.private-git-{index}]", "git = " + json.dumps(url)])
        for key, values in sorted(options.items()):
            if len(values) != 1:
                raise ValueError("ambiguous locked Git source: " + source)
            lines.append(key + " = " + json.dumps(values[0]))
        lines.extend(['replace-with = "private-cargo-sources"', ""])
    lines.extend(['[source.private-cargo-sources]', 'directory = "sources"', ""])
    return "\n".join(lines)


def normalized_member(member):
    member.uid = member.gid = member.mtime = 0
    member.uname = member.gname = ""
    member.pax_headers = {}
    return member


def export(output):
    output = output.resolve()
    build = ROOT / ".wisent-output"
    if build not in output.parents:
        raise ValueError("private source output must be inside " + str(build))
    output.parent.mkdir(parents=True, exist_ok=True)
    lock_digest = digest(ROOT / "Cargo.lock")
    with tempfile.TemporaryDirectory(prefix="cargo-input-", dir=build) as work:
        work = Path(work)
        vendor = work / "vendor"
        run(["cargo", "vendor", "--locked", "--versioned-dirs", "--quiet", str(vendor)])
        metadata = json.loads(run(["cargo", "metadata", "--locked", "--offline",
                                   "--format-version=1"], capture=True).stdout)
        packages = sorted((package for package in metadata["packages"]
                           if (package["source"] or "").startswith("git+")),
                          key=lambda package: (package["name"], package["version"]))
        if not packages:
            raise ValueError("Cargo.lock contains no private Git source packages")
        payload = work / "payload"
        sources = payload / "sources"
        sources.mkdir(parents=True)
        records = []
        for package in packages:
            name = package["name"] + "-" + package["version"]
            source = vendor / name
            if not (source / ".cargo-checksum.json").is_file():
                raise ValueError("Cargo did not vendor checksum-protected source: " + name)
            source.rename(sources / name)
            records.append({key: package[key] for key in ("name", "version", "source")})
        if digest(ROOT / "Cargo.lock") != lock_digest:
            raise ValueError("Cargo.lock changed while exporting private sources")
        provenance = {"schema_version": 1, "cargo_lock_sha256": lock_digest,
                      "packages": records}
        (payload / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
        (payload / "config.toml").write_text(source_config({row["source"] for row in records}))
        archive = work / "private-cargo-sources.tar.gz"
        with archive.open("wb") as raw:
            with gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w") as tar:
                    for path in sorted(payload.iterdir()):
                        tar.add(path, arcname=path.name, filter=normalized_member)
        sha256 = digest(archive)
        os.replace(archive, output)
    revision = run(["git", "rev-parse", "HEAD"], capture=True).stdout.strip()
    receipt = {"source_revision": revision, "archive": str(output), "sha256": sha256,
               "input": {"uri": "stado://sources/jeden/private-cargo-sources/" + sha256
                                  + "/private-cargo-sources.tar.gz",
                         "sha256": sha256, "mount": "private-cargo-sources", "extract": True},
               "provenance": provenance}
    output.with_suffix(output.suffix + ".json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))


def cargo(arguments):
    configured = os.environ.get(INPUT_ENV, "").strip()
    if not configured:
        raise ValueError(INPUT_ENV + " is required for release Cargo commands")
    root = Path(configured).resolve(strict=True)
    provenance = json.loads((root / "provenance.json").read_text())
    if provenance["schema_version"] != 1:
        raise ValueError("unsupported private Cargo source schema")
    if provenance["cargo_lock_sha256"] != digest(ROOT / "Cargo.lock"):
        raise ValueError("private Cargo sources do not match Cargo.lock; export and publish a new input")
    configuration = root / "config.toml"
    expected = source_config({package["source"] for package in provenance["packages"]})
    if configuration.read_text() != expected:
        raise ValueError("private Cargo source configuration does not match its provenance")
    directory = "source.private-cargo-sources.directory=" + json.dumps(str(root / "sources"))
    return subprocess.run(["cargo", "--config", str(configuration), "--config", directory,
                           *arguments], cwd=ROOT).returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    exporter = actions.add_parser("export", help="export Cargo.lock's private sources to an immutable input")
    exporter.add_argument("output", type=Path, help="archive path under .wisent-output")
    consumer = actions.add_parser("cargo", help="run Cargo with the declared private source input")
    consumer.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.action == "export":
        export(args.output)
        return 0
    if not args.arguments:
        parser.error("cargo requires a Cargo command")
    return cargo(args.arguments)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as error:
        print("private Cargo sources: command failed: " + " ".join(error.cmd), file=sys.stderr)
        sys.exit(error.returncode)
    except (OSError, ValueError, KeyError) as error:
        print("private Cargo sources: " + str(error), file=sys.stderr)
        sys.exit(1)
