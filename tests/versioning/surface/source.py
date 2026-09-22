"""One Rust file, scanned once into its string literals and a masked copy.

The mask is the file with comment bodies, character literals and string
contents blanked out, so a search for a declaration cannot match something
that was only ever mentioned inside a comment or a string.
"""

from __future__ import annotations

import pathlib
import re

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

