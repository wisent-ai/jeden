"""The two vocabularies that make up the surface: the commands the binary
dispatches, and the slash commands the capability registry declares."""

from __future__ import annotations

import pathlib

from .arms import match_arm_patterns
from .source import ONE, Source

DISPATCH_FILE = pathlib.PurePosixPath("rust/main.rs")
REGISTRY_FILE = pathlib.PurePosixPath("rust/capability/mod.rs")

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
    """Every builtin slash command and alias, from the capability registry."""
    source = Source(root / REGISTRY_FILE, str(REGISTRY_FILE))
    anchor = source.sole_anchor(
        r"fn\s+builtin_slash_specs\s*\(\s*\)[^{]*(?=\{)", "builtin slash registry"
    )
    body = anchor.end()
    stop = source.balanced_end(body, "{", "}")
    names = []
    for field in source.anchors(r"\bname\s*:\s*(?=\")"):
        if body <= field.start() < stop:
            names.append(source.literal_at(field.end()))
    for field in source.anchors(r"\baliases\s*:\s*&\s*(?=\[)"):
        if body <= field.start() < stop:
            bracket = field.end()
            names.extend(
                source.literals_within(bracket, source.balanced_end(bracket, "[", "]"))
            )
    kept = {name for name in names if name}
    if not kept:
        raise SurfaceError(f"{REGISTRY_FILE}: slash registry yielded no names")
    return sorted(kept)


def surface(root: pathlib.Path) -> list:
    names = [f"cli:{name}" for name in cli_commands(root)]
    names += [f"slash:/{name}" for name in slash_commands(root)]
    return sorted(set(names))

