"""Regenerate released-surface.json from jeden's most recently published artifact.

The baseline a version gate measures against has to come from something a user can
actually obtain, otherwise every later verdict is measured against a tree only
this checkout has. Jeden publishes signed per-platform binaries to GitHub
Releases, so that is the channel this reads. It is deliberately not crates.io: the
`jeden` name there belongs to an unrelated third party (a 967-byte `cargo new`
stub exporting `pub fn add(left, right)`, owner `d14mndz`), so asserting anything
in either direction against crates.io would be false.

What it does, in order:

  * asks GitHub for every release asset named `jeden-<version>-<target>.tar.gz`
    and picks the newest published version, never the version Cargo.toml
    declares. Those differ by design — Cargo.toml holds the SemVer *floor* and
    the pipeline publishes floor-plus-one-patch as a prerelease — and they differ
    again whenever the auto-bump has not landed. Reading the declared version
    would silently measure against an artifact nobody published.
  * takes `version` and `minimumVersion` from that release's signed
    `manifest.dsse.json`, and the exact `sourceSha` from its `channel.json`, so
    every number in the baseline is the artifact's own claim rather than ours.
  * reproduces that revision with `git archive` and extracts its surface with
    scripts/surface.py.

`version` is the prerelease string actually published. `minimumVersion` is the
stable floor that artifact was built from, copied verbatim from its manifest, and
it is what the shared rule receives as `--current`: the rule refuses a version
that is not a major.minor.patch triple, and the floor is the only stable triple
the published evidence names. Recording both is what keeps this file from
claiming a stable `0.1.0` or `0.1.1` was released, because neither ever was.

MARKER_PREFIX is the fleet's machine-readable statement of where a baseline came
from: the first whitespace-delimited token of `source`. It is defined here once
and read back by .github/workflows/version-check.yml, so the two files are
coupled by a constant rather than by prose. The workflow refuses a marker tier
this generator does not implement rather than letting a baseline degrade quietly
to a weaker one, and refuses a marker this generator would no longer produce,
which is what stops a superseded release from being measured against forever.

Usage:
    python3 scripts/baseline.py             # rewrite released-surface.json
    python3 scripts/baseline.py --stdout    # write nothing, print the document
"""

from __future__ import annotations

import base64
import json
import pathlib
import re
import subprocess
import sys
import tempfile

ZERO = int(False)
ONE = int(True)
TWO = ONE + ONE

sys.path.insert(ZERO, str(pathlib.Path(__file__).resolve().parent))

from surface import SurfaceError, surface  # noqa: E402

REPOSITORY = "wisent-ai/jeden"
MARKER_PREFIX = "gh-release:"
BASELINE_FILE = "released-surface.json"
STABLE_TRIPLE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
ASSET = re.compile(r"^jeden-(?P<version>.+?)-(?P<target>[a-z0-9_]+-[a-z0-9_-]+)\.tar\.gz$")
CLAIM_FILES = ("manifest.dsse.json", "channel.json")


def run(command: list, cwd=None) -> str:
    finished = subprocess.run(
        command, capture_output=True, text=True, check=False, cwd=cwd
    )
    if finished.returncode != ZERO:
        detail = finished.stderr.strip() or finished.stdout.strip()
        raise SurfaceError(f"`{' '.join(command)}` failed: {detail}")
    return finished.stdout


def gh(*arguments: str) -> object:
    return json.loads(run(["gh", *arguments, "--repo", REPOSITORY]))


def published_releases() -> list:
    """Releases serving a versioned jeden artifact, newest publication first."""
    listed = gh("release", "list", "--json", "tagName,createdAt")
    found = []
    for release in sorted(listed, key=lambda entry: entry["createdAt"], reverse=True):
        tag = release["tagName"]
        names = [
            asset["name"]
            for asset in gh("release", "view", tag, "--json", "assets")["assets"]
        ]
        versions = {
            matched.group("version")
            for matched in (ASSET.match(name) for name in names)
            if matched
        }
        if len(versions) > ONE:
            raise SurfaceError(f"{tag}: serves more than one version: {sorted(versions)}")
        if versions:
            found.append((tag, versions.pop(), names))
    if not found:
        raise SurfaceError(
            f"{REPOSITORY} serves no jeden-<version>-<target>.tar.gz asset, "
            "so there is no published baseline to recover"
        )
    return found


def canonical_release() -> tuple:
    """The one release this baseline is recovered from, chosen deterministically.

    Jeden publishes one release per target triple, so a single version is served
    by several releases whose only difference is the platform. Taking whichever
    happened to be created last would make the marker depend on the order three
    parallel matrix jobs finished in, and the staleness check compares the whole
    marker — so an unchanged repository could start refusing because a sibling
    release was recreated. Pick the newest published *version*, then the
    lexicographically first of the tags serving it.
    """
    releases = published_releases()
    newest = releases[ZERO][ONE]
    serving = sorted(entry for entry in releases if entry[ONE] == newest)
    return serving[ZERO]


def artifact_claims(tag: str, names: list, scratch: pathlib.Path) -> dict:
    """The release's own statements: published version, floor, source revision."""
    missing = [name for name in CLAIM_FILES if name not in names]
    if missing:
        raise SurfaceError(
            f"{tag}: no {', '.join(missing)}, so its version cannot be trusted"
        )
    patterns = []
    for name in CLAIM_FILES:
        patterns += ["--pattern", name]
    run(["gh", "release", "download", tag, "--repo", REPOSITORY, *patterns,
         "--dir", str(scratch)])
    envelope = json.loads((scratch / "manifest.dsse.json").read_text(encoding="utf-8"))
    manifest = json.loads(base64.b64decode(envelope["payload"]))
    channel = json.loads((scratch / "channel.json").read_text(encoding="utf-8"))
    claims = {
        "version": manifest.get("version"),
        "minimumVersion": manifest.get("minimumVersion"),
        "sourceSha": channel.get("sourceSha"),
    }
    for key, value in claims.items():
        if not isinstance(value, str) or not value.strip():
            raise SurfaceError(f"{tag}: {key} is missing from the published evidence")
    if not STABLE_TRIPLE.match(claims["minimumVersion"]):
        raise SurfaceError(
            f"{tag}: minimumVersion {claims['minimumVersion']!r} is not a stable triple, "
            "so the shared rule would have no version slot to advance"
        )
    return claims


def published_surface(root: pathlib.Path, source_sha: str, scratch: pathlib.Path) -> list:
    """The surface of the exact revision the published artifact was built from."""
    kind = run(["git", "cat-file", "-t", source_sha], cwd=root).strip()
    if kind != "commit":
        raise SurfaceError(f"{source_sha} is a {kind}, not a commit")
    tree = scratch / "tree"
    tree.mkdir()
    archive = subprocess.run(
        ["git", "archive", source_sha], cwd=root, capture_output=True, check=False
    )
    if archive.returncode != ZERO:
        detail = archive.stderr.decode(errors="replace").strip()
        raise SurfaceError(f"cannot reproduce {source_sha}: {detail}")
    subprocess.run(["tar", "-x", "-C", str(tree)], input=archive.stdout, check=True)
    return surface(tree)


def build(root: pathlib.Path) -> dict:
    tag, version, names = canonical_release()
    with tempfile.TemporaryDirectory() as scratch_name:
        scratch = pathlib.Path(scratch_name)
        claims = artifact_claims(tag, names, scratch)
        if claims["version"] != version:
            raise SurfaceError(
                f"{tag}: asset is named {version} but its signed manifest says "
                f"{claims['version']}"
            )
        recovered = published_surface(root, claims["sourceSha"], scratch)
    return {
        "version": version,
        "minimumVersion": claims["minimumVersion"],
        "source": (
            f"{MARKER_PREFIX}{tag}"
            f" surface reproduced with `git archive` from {claims['sourceSha']},"
            f" the sourceSha that release's channel.json declares; version and"
            f" minimumVersion copied from its signed manifest.dsse.json"
        ),
        "surface": recovered,
    }


def best_marker() -> str:
    """The marker this generator would produce right now, without rebuilding.

    The staleness check in CI only needs to know *which* release is newest, so it
    must not reproduce a source revision: a shallow CI checkout does not have the
    published commit, and regenerating the surface at check time is the one shape
    that structurally cannot refuse. Resolving the marker alone keeps the frozen
    committed surface the only surface a decision ever sees.
    """
    tag, _version, _names = canonical_release()
    return f"{MARKER_PREFIX}{tag}"


def main(argv: list) -> int:
    root = pathlib.Path(__file__).resolve().parent.parent
    if "--marker" in argv:
        print(best_marker())
        return int(False)
    document = build(root)
    text = json.dumps(document, indent=TWO) + "\n"
    if "--stdout" in argv:
        sys.stdout.write(text)
        return int(False)
    (root / BASELINE_FILE).write_text(text, encoding="utf-8")
    sys.stderr.write(
        f"{BASELINE_FILE}: {document['version']} "
        f"(floor {document['minimumVersion']}), {len(document['surface'])} names\n"
    )
    return int(False)


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[ONE:]))
    except SurfaceError as error:
        sys.stderr.write(f"error: {error}\n")
        sys.exit(int(True))
