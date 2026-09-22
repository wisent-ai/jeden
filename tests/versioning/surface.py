"""Print jeden's public surface: the command vocabulary the binary advertises.

Jeden is not a library — nothing links against it, and its Rust `lib.rs` exports
one line. What a caller actually depends on is **which commands the binary
answers to**: the `jeden <command>` subcommands a script or the ACP/VS Code
clients invoke, and the `/command` names an interactive user types. Adding one is
a capability; removing one breaks whoever scripted it yesterday. So the command
vocabulary is the public contract, and this prints it for the shared versioning
rule to compare.

Two families, each namespaced because a CLI subcommand and a slash command of the
same word are different promises reached through different code (`jeden export`
vs `/export`):

    cli:<name>      a command the binary dispatches
    slash:<name>    a builtin slash command, including its aliases

Each family is read from the one place that actually decides it:

  * `cli:` from the dispatcher in `rust/main.rs` — the `match args.command
    .as_str()` arms, the `args.command == "..."` checks that short-circuit ahead
    of it, and the `matches!(command.as_str(), ...)` pre-dispatch in
    `parse_args`. Behaviour, not documentation: `usage()` is prose that can and
    does drift from the dispatcher (it omits `collab-relay`, `update` and
    `tools`), and a command keeps working when its help line is deleted.
  * `slash:` from `builtin_slash_specs()` in `rust/capability/mod.rs` — the
    static registry that feeds `/help`, `jeden capabilities` and the pickers.
    Aliases count: `/models` is what a user types, so losing it is a break.

Options (`--json`, `--cwd`) are deliberately excluded. Jeden models its own
capabilities with `CapabilityKind`, and a flag is not one of them: flags modify a
command, commands are the named things the product offers. That line comes from
the repository's own model rather than from taste.

Read statically, never by building. Nothing here runs `cargo`, so a release
decision cannot depend on a machine having a Rust toolchain or on a crate index
being reachable. It also means this runs unchanged against a tree unpacked from a
published artifact's source revision, so the surface of an already published
version can be recovered exactly rather than assumed.

A file that does not parse, or a declaration site that has moved, raises. It
never degrades to a smaller surface: silently dropping half the commands would
read as a clean removal and mislabel the release.

Usage:
    python3 - [root]     # root defaults to the repository
"""

from __future__ import annotations

import json
import pathlib
import sys

from surface import surface

ZERO = int(False)
ONE = int(True)
TWO = ONE + ONE


def main(argv: list) -> int:
    root = pathlib.Path(argv[ZERO]) if argv else pathlib.Path(__file__).resolve().parents[TWO]
    print(json.dumps({"surface": surface(root)}, indent=TWO))
    return ZERO


if __name__ == "__main__":
    sys.exit(main(sys.argv[ONE:]))
