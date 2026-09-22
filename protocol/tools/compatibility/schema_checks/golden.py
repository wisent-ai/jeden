"""Validating a document against the subset of JSON Schema the protocol uses,
and holding the golden envelopes to it."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Mapping, Sequence

from ..contract import KINDS, CheckFailure, Document
from ..documents import _json_pointer, _schema_owner, _walk

def _validate(value: Any, schema: Any, root: Any, location: str) -> list[str]:
    """Validate the Draft 2020-12 subset used by the canonical schemas."""
    if schema is True:
        return []
    if schema is False:
        return [f"{location}: schema is false"]
    if not isinstance(schema, dict):
        return [f"{location}: invalid schema node {schema!r}"]
    if "$ref" in schema:
        try:
            target = _json_pointer(root, schema["$ref"])
        except CheckFailure as exc:
            return [f"{location}: {exc}"]
        return _validate(value, target, root, location)
    failures: list[str] = []
    if "allOf" in schema:
        failures.extend(
            item for child in schema["allOf"]
            for item in _validate(value, child, root, location)
        )
    if "oneOf" in schema:
        outcomes = [_validate(value, child, root, location) for child in schema["oneOf"]]
        if sum(not outcome for outcome in outcomes) != 1:
            failures.append(f"{location}: expected exactly one oneOf alternative to match")
    if "anyOf" in schema:
        outcomes = [_validate(value, child, root, location) for child in schema["anyOf"]]
        if not any(not outcome for outcome in outcomes):
            failures.append(f"{location}: no anyOf alternative matched")
    if "if" in schema:
        condition_matches = not _validate(value, schema["if"], root, location)
        branch = schema.get("then") if condition_matches else schema.get("else")
        if branch is not None:
            failures.extend(_validate(value, branch, root, location))
    if "const" in schema and value != schema["const"]:
        failures.append(f"{location}: expected constant {schema['const']!r}, got {value!r}")
    if "enum" in schema and value not in schema["enum"]:
        failures.append(f"{location}: {value!r} is not in enum {schema['enum']!r}")

    expected_type = schema.get("type")
    type_ok = {
        "object": isinstance(value, dict),
        "array": isinstance(value, list),
        "string": isinstance(value, str),
        "integer": isinstance(value, int) and not isinstance(value, bool),
        "number": isinstance(value, (int, float)) and not isinstance(value, bool),
        "boolean": isinstance(value, bool),
        "null": value is None,
    }.get(expected_type, True)
    if not type_ok:
        failures.append(f"{location}: expected type {expected_type}, got {type(value).__name__}")
        return failures

    # Combinator failures above are combined with ordinary sibling keywords.
    if isinstance(value, dict):
        required = schema.get("required", [])
        for name in required:
            if name not in value:
                failures.append(f"{location}: missing required property {name!r}")
        properties = schema.get("properties", {})
        for name, child in value.items():
            child_location = f"{location}.{name}"
            if name in properties:
                failures.extend(_validate(child, properties[name], root, child_location))
            elif schema.get("additionalProperties") is False:
                failures.append(f"{location}: unknown property {name!r}")
            elif isinstance(schema.get("additionalProperties"), dict):
                failures.extend(_validate(child, schema["additionalProperties"], root, child_location))
    if isinstance(value, list) and isinstance(schema.get("items"), dict):
        for index, child in enumerate(value):
            failures.extend(_validate(child, schema["items"], root, f"{location}[{index}]"))
    if isinstance(value, str):
        if "minLength" in schema and len(value) < schema["minLength"]:
            failures.append(f"{location}: string is shorter than minLength {schema['minLength']}")
        pattern = schema.get("pattern")
        if isinstance(pattern, str) and re.compile(pattern).search(value) is None:
            failures.append(f"{location}: string does not match pattern {pattern!r}")
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if "minimum" in schema and value < schema["minimum"]:
            failures.append(f"{location}: value is less than minimum {schema['minimum']}")
        if "maximum" in schema and value > schema["maximum"]:
            failures.append(f"{location}: value is greater than maximum {schema['maximum']}")
    return failures


def _golden_envelopes(documents: Sequence[Document]) -> list[tuple[Path, str, dict[str, Any]]]:
    found: list[tuple[Path, str, dict[str, Any]]] = []
    for document in documents:
        if isinstance(document.value, dict) and "$schema" in document.value:
            continue
        for node in _walk(document.value):
            if isinstance(node, dict) and node.get("type") in KINDS:
                found.append((document.path, f"{document.path.name}:{node['type']}", node))
    return found


def _check_golden(
    documents: Sequence[Document], schemas: Mapping[str, dict[str, Any]], errors: list[str]
) -> None:
    envelopes = _golden_envelopes(documents)
    seen = {kind for _, _, envelope in envelopes for kind in [envelope["type"]]}
    for kind in KINDS:
        if kind not in seen:
            errors.append(f"golden fixtures: missing '{kind}' envelope")
    for path, label, envelope in envelopes:
        kind = envelope["type"]
        schema = schemas.get(kind)
        if schema is None:
            continue
        owner = _schema_owner(documents, schema)
        failures = _validate(envelope, schema, owner, label)
        errors.extend(f"golden validation: {failure}" for failure in failures)


