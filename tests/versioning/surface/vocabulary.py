"""The two vocabularies that make up the surface: the commands the binary
dispatches, and the slash commands the capability registry declares."""

from __future__ import annotations

import json
import pathlib

from .arms import match_arm_patterns
from .source import ONE, Source, SurfaceError

DISPATCH_FILE = pathlib.PurePosixPath("rust/main.rs")
# The builtin slash commands are a declaration the binary compiles in with
# `include_str!` (rust/capability/builtin/mod.rs), not Rust source: the split
# of the capability registry moved them there, and every version check since
# failed with "expected exactly one builtin slash registry, found 0" because
# this reader still searched rust/capability/mod.rs for a function body.
REGISTRY_FILE = pathlib.PurePosixPath("rust/capability/builtin/builtin-slash-commands.json")

def cli_commands(root: pathlib.Path) -> list:
    """Every command name the binary dispatches, from rust/main.rs."""
    source = Source(root / DISPATCH_FILE, str(DISPATCH_FILE))
    anchor = source.sole_anchor(
        r"match\s+args\.command\.as_str\(\)\s*(?=\{)", "command dispatcher"
    )
    block = anchor.end()
    names = match_arm_patterns(source, block + ONE, source.balanced_end(block, "{", "}"))
    for equality in source.anchors(r"\bcommand\s*==\s*(?=\")"):
        names.append(source.literal_at(equality.end()))
    for guard in source.anchors(
        r"matches!\s*\(\s*(?:args\.)?command\.as_str\(\)\s*,\s*"
    ):
        opener = guard.start() + guard.group().index("(")
        names.extend(source.literals_within(guard.end(), source.balanced_end(opener, "(", ")")))
    kept = {name for name in names if name and not name.startswith("-")}
    if not kept:
        raise SurfaceError(f"{DISPATCH_FILE}: dispatcher yielded no command names")
    return sorted(kept)


def slash_commands(root: pathlib.Path) -> list:
    """Every builtin slash command and alias, from the catalogue the binary
    compiles in."""
    path = root / REGISTRY_FILE
    try:
        catalogue = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise SurfaceError(f"{REGISTRY_FILE}: cannot read the slash catalogue: {error}") from error
    if not isinstance(catalogue, list):
        raise SurfaceError(f"{REGISTRY_FILE}: the slash catalogue is not a list")
    names = []
    for entry in catalogue:
        if not isinstance(entry, dict) or not isinstance(entry.get("name"), str):
            raise SurfaceError(f"{REGISTRY_FILE}: an entry carries no name: {entry!r}")
        names.append(entry["name"])
        aliases = entry.get("aliases", [])
        if not isinstance(aliases, list) or not all(isinstance(alias, str) for alias in aliases):
            raise SurfaceError(f"{REGISTRY_FILE}: {entry['name']} carries malformed aliases")
        names.extend(aliases)
    kept = {name for name in names if name}
    if not kept:
        raise SurfaceError(f"{REGISTRY_FILE}: slash registry yielded no names")
    return sorted(kept)


def surface(root: pathlib.Path) -> list:
    names = [f"cli:{name}" for name in cli_commands(root)]
    names += [f"slash:/{name}" for name in slash_commands(root)]
    return sorted(set(names))

