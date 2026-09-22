"""The SDK surfaces the protocol promises, and the manifest a run records:
which files were read, and the digest of each one."""

from __future__ import annotations

import hashlib
import re
from pathlib import Path
from typing import Any, Mapping, Sequence

from .contract import (
    CAMEL_FIELDS,
    KINDS,
    PROTOCOL_ID,
    REQUIRED_FIELDS,
    RUST_FIELDS,
    Document,
)

def _source_files(directory: Path, suffixes: tuple[str, ...]) -> list[Path]:
    if not directory.is_dir():
        return []
    return sorted(
        path for path in directory.rglob("*")
        if path.is_file() and path.suffix in suffixes
    )


def _read_sources(paths: Sequence[Path], errors: list[str], language: str) -> str:
    chunks: list[str] = []
    for path in paths:
        try:
            chunks.append(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError) as exc:
            errors.append(f"{language} SDK: cannot read {path}: {exc}")
    return "\n".join(chunks)


def _check_sdks(root: Path, errors: list[str]) -> dict[str, list[Path]]:
    groups = {
        "rust": _source_files(root / "rust" / "sdk", (".rs",)),
        "typescript": _source_files(root / "packages" / "sdk-typescript", (".ts", ".tsx")),
        "python": _source_files(root / "python" / "jeden_sdk", (".py",)),
    }
    for language, paths in groups.items():
        if not paths:
            errors.append(f"{language} SDK: no source files found at its canonical SDK path")
            continue
        source = _read_sources(paths, errors, language)
        if PROTOCOL_ID not in source:
            errors.append(f"{language} SDK: protocol constant value {PROTOCOL_ID!r} not found")
            continue
        if language == "rust":
            if not re.search(
                r"(?m)\b(?:pub(?:\([^)]*\))?\s+)?const\s+(?:PROTOCOL[A-Z0-9_]*|[A-Z][A-Z0-9_]*_PROTOCOL[A-Z0-9_]*)\s*:\s*&(?:'static\s+)?str\s*=\s*\"jeden\.session\.v1\"",
                source,
            ):
                errors.append("rust SDK: expected a named public/protocol const equal to 'jeden.session.v1'")
            rename_all = bool(re.search(r"rename_all\s*=\s*\"camelCase\"", source))
            for camel, snake in RUST_FIELDS.items():
                if snake not in source:
                    errors.append(f"rust SDK: field {snake!r} (JSON {camel!r}) not found")
                if not rename_all and f'"{camel}"' not in source:
                    errors.append(
                        f"rust SDK: no serde camelCase policy or explicit spelling {camel!r}"
                    )
        elif language == "typescript":
            if not re.search(
                r"(?m)\b(?:export\s+)?const\s+(?:PROTOCOL[A-Z0-9_]*|[A-Z][A-Z0-9_]*_PROTOCOL[A-Z0-9_]*)\s*(?::[^=]+)?=\s*['\"]jeden\.session\.v1['\"]",
                source,
            ):
                errors.append("typescript SDK: expected a protocol const equal to 'jeden.session.v1'")
            for field in CAMEL_FIELDS:
                if not re.search(rf"\b{re.escape(field)}\b", source):
                    errors.append(f"typescript SDK: JSON field spelling {field!r} not found")
        else:
            if not re.search(
                r"(?m)^(?:PROTOCOL[A-Z0-9_]*|[A-Z][A-Z0-9_]*_PROTOCOL[A-Z0-9_]*)\s*(?::[^=]+)?=\s*['\"]jeden\.session\.v1['\"]",
                source,
            ):
                errors.append("python SDK: expected a module protocol constant equal to 'jeden.session.v1'")
            for field in CAMEL_FIELDS:
                if field not in source:
                    errors.append(f"python SDK: serialized JSON field spelling {field!r} not found")
    return groups


def _manifest(root: Path, documents: Sequence[Document], groups: Mapping[str, Sequence[Path]]) -> dict[str, Any]:
    def digest(path: Path) -> str:
        return hashlib.sha256(path.read_bytes()).hexdigest()

    return {
        "protocol": PROTOCOL_ID,
        "envelopes": {kind: list(REQUIRED_FIELDS[kind]) for kind in KINDS},
        "json": [
            {"path": str(document.path.relative_to(root)), "sha256": digest(document.path)}
            for document in sorted(documents, key=lambda item: str(item.path))
        ],
        "sdkSources": {
            language: [str(path.relative_to(root)) for path in paths]
            for language, paths in sorted(groups.items())
        },
    }


