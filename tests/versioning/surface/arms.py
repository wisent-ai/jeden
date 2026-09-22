"""Reading the arms of a Rust match block: where each arm begins, and the
literals in its pattern. This is how the command vocabulary is recovered from
the dispatcher itself rather than from a list somebody has to remember to
update."""

from __future__ import annotations

from .source import NOT_FOUND, ONE, TWO, ZERO, Source, SurfaceError

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


