"""Reading the schema documents: loading them, walking them, and finding the
one schema that describes a given kind of envelope."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Iterable, Sequence

from .contract import CheckFailure, Document

def _json_files(directory: Path) -> list[Path]:
    return sorted(p for p in directory.rglob("*.json") if p.is_file())


def _load_documents(directory: Path, errors: list[str]) -> list[Document]:
    documents: list[Document] = []
    if not directory.is_dir():
        errors.append(f"schema directory is missing: {directory}")
        return documents
    for path in _json_files(directory):
        try:
            with path.open("r", encoding="utf-8") as handle:
                documents.append(Document(path, json.load(handle)))
        except (OSError, UnicodeError, json.JSONDecodeError) as exc:
            errors.append(f"cannot read JSON {path}: {exc}")
    return documents


def _walk(value: Any) -> Iterable[Any]:
    yield value
    if isinstance(value, dict):
        for child in value.values():
            yield from _walk(child)
    elif isinstance(value, list):
        for child in value:
            yield from _walk(child)


def _object_schema_for_kind(documents: Sequence[Document], kind: str) -> dict[str, Any] | None:
    candidates: list[dict[str, Any]] = []
    for document in documents:
        for node in _walk(document.value):
            if not isinstance(node, dict):
                continue
            properties = node.get("properties")
            if not isinstance(properties, dict):
                continue
            discriminator = properties.get("type")
            if isinstance(discriminator, dict) and discriminator.get("const") == kind:
                candidates.append(node)
    if not candidates:
        return None
    return max(candidates, key=lambda item: len(item.get("properties", {})))


def _schema_with_property_const(
    documents: Sequence[Document], property_name: str, constant: str
) -> dict[str, Any] | None:
    candidates: list[dict[str, Any]] = []
    for document in documents:
        for node in _walk(document.value):
            if not isinstance(node, dict):
                continue
            properties = node.get("properties")
            if not isinstance(properties, dict):
                continue
            prop = properties.get(property_name)
            if isinstance(prop, dict) and prop.get("const") == constant:
                candidates.append(node)
    return max(candidates, key=lambda item: len(item.get("properties", {})), default=None)

def _schema_owner(documents: Sequence[Document], schema: Any) -> Any:
    for document in documents:
        if any(node is schema for node in _walk(document.value)):
            return document.value
    return schema


def _resolved_schema(documents: Sequence[Document], schema: Any) -> Any:
    root = _schema_owner(documents, schema)
    seen: set[str] = set()
    while isinstance(schema, dict) and isinstance(schema.get("$ref"), str):
        reference = schema["$ref"]
        if reference in seen:
            return schema
        seen.add(reference)
        try:
            schema = _json_pointer(root, reference)
        except CheckFailure:
            return schema
    return schema


def _object_schema_with_properties(
    documents: Sequence[Document], names: set[str]
) -> dict[str, Any] | None:
    for document in documents:
        for node in _walk(document.value):
            if (
                isinstance(node, dict)
                and node.get("type") == "object"
                and isinstance(node.get("properties"), dict)
                and set(node["properties"]) == names
            ):
                return node
    return None



def _json_pointer(document: Any, fragment: str) -> Any:
    if fragment[:1] != "#":
        raise CheckFailure(f"unsupported non-local reference {fragment!r}")
    current = document
    pointer = fragment[1:]
    if not pointer:
        return current
    if pointer[:1] != "/":
        raise CheckFailure(f"unsupported JSON pointer {fragment!r}")
    for raw in pointer[1:].split("/"):
        token = raw.replace("~1", "/").replace("~0", "~")
        try:
            current = current[int(token)] if isinstance(current, list) else current[token]
        except (IndexError, KeyError, ValueError, TypeError) as exc:
            raise CheckFailure(f"unresolved JSON pointer {fragment!r}") from exc
    return current

