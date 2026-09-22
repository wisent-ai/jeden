"""Jeden-s public surface, assembled from the two vocabularies."""

from __future__ import annotations

import pathlib

from .source import SurfaceError
from .vocabulary import cli_commands, slash_commands


def surface(root: pathlib.Path) -> list:
    names = [f"cli:{name}" for name in cli_commands(root)]
    names += [f"slash:/{name}" for name in slash_commands(root)]
    return sorted(set(names))
