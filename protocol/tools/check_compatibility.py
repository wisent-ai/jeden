#!/usr/bin/env python3
"""Check the canonical jeden.session.v1 schema, golden vectors, and SDK surfaces.

This tool deliberately uses only the Python standard library.  Run it from any
working directory; by default it derives the repository root from this file.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Sequence

from compatibility import check


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    default_root = Path(__file__).resolve().parents[2]
    parser.add_argument("--root", type=Path, default=default_root, help="repository root")
    parser.add_argument(
        "--print-manifest", action="store_true",
        help="print the deterministic language-neutral manifest after a successful check",
    )
    args = parser.parse_args(argv)
    errors, manifest = check(args.root)
    if errors:
        print(f"jeden.session.v1 compatibility check failed ({len(errors)} issue(s)):", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        print(
            f"Invocation: {Path(__file__).name} --root {args.root.resolve()}", file=sys.stderr
        )
        return 1
    if args.print_manifest:
        print(json.dumps(manifest, indent=2, sort_keys=True) + "\n", end="")
    else:
        print(
            f"jeden.session.v1 compatibility check passed: "
            f"{len(manifest['json'])} JSON document(s), "
            f"{sum(len(items) for items in manifest['sdkSources'].values())} SDK source file(s)"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
