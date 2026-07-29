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
    python3 scripts/surface.py [root]     # root defaults to the repository
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ZERO = int(False)
ONE = int(True)
TWO = ONE + ONE
NOT_FOUND = -ONE

DISPATCH_FILE = pathlib.PurePosixPath("rust/main.rs")
REGISTRY_FILE = pathlib.PurePosixPath("rust/capability/mod.rs")

IDENT = re.compile(r"[A-Za-z0-9_]")
ESCAPES = {
    "n": "\n",
    "t": "\t",
    "r": "\r",
    "0": "\0",
    "\\": "\\",
    '"': '"',
    "'": "'",
}


class SurfaceError(Exception):
    """A declaration site is missing or unreadable, so the surface is unknown."""


class Source:
    """One Rust file, scanned once into string literals plus a masked copy.

    `mask` is the file with comment bodies, char literals and string *contents*
    replaced by spaces, keeping the quotes. Offsets stay aligned with the
    original, so braces and brackets can be matched without a `{` inside a
    string throwing the depth off, and an anchor regex stops dead at the opening
    quote of the value it introduces.
    """

    def __init__(self, path: pathlib.Path, label: str) -> None:
        self.label = label
        try:
            self.text = path.read_text(encoding="utf-8")
        except OSError as error:
            raise SurfaceError(f"cannot read {label}: {error}") from error
        self.literals: list = []
        self.mask = self._scan()

    def _scan(self) -> str:
        text = self.text
        size = len(text)
        mask = list(text)
        index = ZERO
        while index < size:
            char = text[index]
            following = text[index + ONE] if index + ONE < size else ""
            if char == "/" and following == "/":
                stop = text.find("\n", index)
                stop = size if stop == NOT_FOUND else stop
                index = self._blank(mask, index, stop)
                continue
            if char == "/" and following == "*":
                index = self._blank(mask, index, self._block_comment_end(index))
                continue
            if char == "'":
                index = self._blank(mask, index, self._quote_or_lifetime_end(index))
                continue
            raw = self._raw_string_span(index)
            if raw is not None:
                open_end, close_start, close_end = raw
                self.literals.append((index, close_end, text[open_end:close_start]))
                self._blank(mask, open_end, close_start)
                index = close_end
                continue
            if char == '"':
                index = self._plain_string(mask, index)
                continue
            index += ONE
        return "".join(mask)

    @staticmethod
    def _blank(mask: list, start: int, stop: int) -> int:
        for position in range(start, stop):
            if mask[position] != "\n":
                mask[position] = " "
        return stop

    def _block_comment_end(self, start: int) -> int:
        text = self.text
        size = len(text)
        depth = ONE
        index = start + TWO
        while index < size and depth > ZERO:
            if text.startswith("/*", index):
                depth += ONE
                index += TWO
            elif text.startswith("*/", index):
                depth -= ONE
                index += TWO
            else:
                index += ONE
        if depth > ZERO:
            raise SurfaceError(f"{self.label}: unterminated block comment")
        return index

    def _quote_or_lifetime_end(self, start: int) -> int:
        """End of a char literal, or of a lifetime such as `'static`."""
        text = self.text
        size = len(text)
        index = start + ONE
        if index < size and text[index] == "\\":
            index += TWO
            while index < size and text[index] != "'":
                index += ONE
            if index >= size:
                raise SurfaceError(f"{self.label}: unterminated char literal")
            return index + ONE
        run = index
        while run < size and IDENT.match(text[run]):
            run += ONE
        if run == index + ONE and run < size and text[run] == "'":
            return run + ONE
        return run

    def _raw_string_span(self, start: int):
        """Span of a raw/byte string starting at `start`, or None."""
        text = self.text
        size = len(text)
        if start > ZERO and IDENT.match(text[start - ONE]):
            return None
        index = start
        if index < size and text[index] == "b":
            index += ONE
        if index >= size or text[index] != "r":
            return None
        index += ONE
        hashes = ZERO
        while index < size and text[index] == "#":
            hashes += ONE
            index += ONE
        if index >= size or text[index] != '"':
            return None
        open_end = index + ONE
        terminator = '"' + "#" * hashes
        close_start = text.find(terminator, open_end)
        if close_start == NOT_FOUND:
            raise SurfaceError(f"{self.label}: unterminated raw string")
        return open_end, close_start, close_start + len(terminator)

    def _plain_string(self, mask: list, start: int) -> int:
        text = self.text
        size = len(text)
        index = start + ONE
        pieces = []
        while index < size:
            char = text[index]
            if char == "\\":
                if index + ONE >= size:
                    break
                pieces.append(ESCAPES.get(text[index + ONE], text[index + ONE]))
                index += TWO
                continue
            if char == '"':
                self.literals.append((start, index + ONE, "".join(pieces)))
                self._blank(mask, start + ONE, index)
                return index + ONE
            pieces.append(char)
            index += ONE
        raise SurfaceError(f"{self.label}: unterminated string literal")

    def literal_at(self, offset: int) -> str:
        for start, _stop, value in self.literals:
            if start == offset:
                return value
        raise SurfaceError(f"{self.label}: expected a string literal at offset {offset}")

    def literals_within(self, start: int, stop: int) -> list:
        return [value for begin, _end, value in self.literals if start <= begin < stop]

    def anchors(self, pattern: str) -> list:
        return list(re.finditer(pattern, self.mask))

    def sole_anchor(self, pattern: str, what: str):
        found = self.anchors(pattern)
        if len(found) != ONE:
            raise SurfaceError(
                f"{self.label}: expected exactly one {what}, found {len(found)}"
            )
        return found[ZERO]

    def balanced_end(self, start: int, opener: str, closer: str) -> int:
        """Offset just past the `closer` matching the `opener` at `start`."""
        if self.mask[start] != opener:
            raise SurfaceError(f"{self.label}: expected {opener!r} at offset {start}")
        depth = ZERO
        for index in range(start, len(self.mask)):
            char = self.mask[index]
            if char == opener:
                depth += ONE
            elif char == closer:
                depth -= ONE
                if depth == ZERO:
                    return index + ONE
        raise SurfaceError(f"{self.label}: unbalanced {opener!r} at offset {start}")


OPENERS = {"{": "}", "(": ")", "[": "]"}
CLOSERS = {"}": "{", ")": "(", "]": "["}


def match_arm_patterns(source: Source, start: int, stop: int) -> list:
    """Literals in the pattern of every arm of the match block spanning start..stop.

    Splits arms by scanning for `=>` at the block's own nesting depth, then takes
    only the literals that sit at that same depth inside the pattern. A literal
    in an arm *body* is nested inside the body's braces or parens, so it cannot
    be mistaken for a pattern.
    """
    mask = source.mask
    patterns = []
    depth = ZERO
    depths = []
    boundary = start
    index = start
    while index < stop:
        char = mask[index]
        if char in OPENERS:
            depth += ONE
            index += ONE
            continue
        if char in CLOSERS:
            depth -= ONE
            index += ONE
            continue
        if depth == ZERO and mask.startswith("=>", index):
            patterns.append((boundary, index))
            body = index + TWO
            while body < stop and mask[body] == " ":
                body += ONE
            if body < stop and mask[body] == "{":
                body = source.balanced_end(body, "{", "}")
                while body < stop and mask[body] in " ,\n":
                    body += ONE
                boundary = body
                index = body
                continue
            inner = ZERO
            while body < stop:
                token = mask[body]
                if token in OPENERS:
                    inner += ONE
                elif token in CLOSERS:
                    inner -= ONE
                elif token == "," and inner == ZERO:
                    break
                body += ONE
            boundary = body + ONE
            index = boundary
            continue
        index += ONE
    del depths
    names = []
    for begin, end in patterns:
        for offset, _stop, value in source.literals:
            if begin <= offset < end and pattern_depth(mask, begin, offset) == ZERO:
                names.append(value)
    return names


def pattern_depth(mask: str, begin: int, offset: int) -> int:
    depth = ZERO
    for index in range(begin, offset):
        char = mask[index]
        if char in OPENERS:
            depth += ONE
        elif char in CLOSERS:
            depth -= ONE
    return depth


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


def main(argv: list) -> int:
    root = pathlib.Path(argv[ZERO]) if argv else pathlib.Path(__file__).resolve().parent.parent
    print(json.dumps({"surface": surface(root)}, indent=TWO))
    return int(False)


if __name__ == "__main__":
    sys.exit(main(sys.argv[ONE:]))
