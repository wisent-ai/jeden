#!/usr/bin/env python3
"""Check the canonical jeden.session.v1 schema, golden vectors, and SDK surfaces.

This tool deliberately uses only the Python standard library.  Run it from any
working directory; by default it derives the repository root from this file.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

PROTOCOL_ID = "jeden.session.v1"
KINDS = ("request", "response", "event", "error")
REQUIRED_FIELDS = {
    "request": ("type", "id", "method", "params", "meta"),
    "response": ("type", "id", "result"),
    "event": (
        "type", "sessionId", "streamId", "sequence", "cursor", "eventId",
        "kind", "payload",
    ),
    "error": ("type", "error"),
}
META_REQUIRED = ("protocolVersion", "idempotencyKey")
META_OPTIONAL = ("deadline", "traceId")
ERROR_REQUIRED = ("code", "message", "retryable", "details")
REPLAY_REQUIRED = ("sessionId",)
REPLAY_OPTIONAL = ("cursor", "limit")
CAMEL_FIELDS = (
    "protocolVersion", "idempotencyKey", "deadline", "traceId", "sessionId",
    "streamId", "sequence", "cursor", "eventId", "requestId", "retryable",
    "details",
)
RUST_FIELDS = {
    "protocolVersion": "protocol_version",
    "idempotencyKey": "idempotency_key",
    "traceId": "trace_id",
    "sessionId": "session_id",
    "streamId": "stream_id",
    "eventId": "event_id",
    "requestId": "request_id",
}


@dataclass(frozen=True)
class Document:
    path: Path
    value: Any


class CheckFailure(Exception):
    pass


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


def _check_exact_object(
    schema: Any,
    *,
    label: str,
    required: Sequence[str],
    optional: Sequence[str] = (),
    errors: list[str],
) -> None:
    if not isinstance(schema, dict):
        errors.append(f"{label}: expected an object schema")
        return
    properties = schema.get("properties")
    actual_required = schema.get("required")
    if schema.get("type") != "object":
        errors.append(f"{label}: type must be 'object'")
    if schema.get("additionalProperties") is not False:
        errors.append(f"{label}: additionalProperties must be false")
    if not isinstance(properties, dict):
        errors.append(f"{label}: properties must be an object")
        return
    expected_properties = set(required) | set(optional)
    if set(properties) != expected_properties:
        errors.append(
            f"{label}: properties are {sorted(properties)}, expected {sorted(expected_properties)}"
        )
    if not isinstance(actual_required, list) or set(actual_required) != set(required):
        errors.append(f"{label}: required is {actual_required!r}, expected {list(required)!r}")


def _check_nonempty(schema: Any, label: str, errors: list[str]) -> None:
    if not isinstance(schema, dict) or schema.get("type") != "string" or schema.get("minLength") != 1:
        errors.append(f"{label}: must be a string schema with minLength 1")

def _check_arbitrary(schema: Any, label: str, errors: list[str]) -> None:
    if schema != {}:
        errors.append(f"{label}: must accept arbitrary JSON (expected an empty schema)")

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



def _check_schema_contract(
    documents: Sequence[Document], errors: list[str]
) -> dict[str, dict[str, Any]]:
    schemas: dict[str, dict[str, Any]] = {}
    for kind in KINDS:
        schema = _object_schema_for_kind(documents, kind)
        if schema is None:
            errors.append(f"schema: no '{kind}' envelope with properties.type.const == '{kind}'")
            continue
        schemas[kind] = schema
        optional = ("requestId",) if kind == "event" else (("id",) if kind == "error" else ())
        _check_exact_object(
            schema, label=f"schema {kind} envelope", required=REQUIRED_FIELDS[kind],
            optional=optional, errors=errors,
        )
        discriminator = schema.get("properties", {}).get("type")
        if not isinstance(discriminator, dict) or discriminator.get("const") != kind:
            errors.append(f"schema {kind} envelope: invalid type discriminator")

    request = schemas.get("request")
    if request:
        properties = request.get("properties", {})
        for name in ("id", "method"):
            _check_nonempty(_resolved_schema(documents, properties.get(name)), f"schema request.{name}", errors)
        _check_arbitrary(properties.get("params"), "schema request.params", errors)
        meta = _resolved_schema(documents, properties.get("meta"))
        _check_exact_object(
            meta, label="schema request.meta", required=META_REQUIRED,
            optional=META_OPTIONAL, errors=errors,
        )
        if isinstance(meta, dict):
            version = meta.get("properties", {}).get("protocolVersion")
            if not isinstance(version, dict) or version.get("const") != PROTOCOL_ID:
                errors.append(
                    f"schema request.meta.protocolVersion: const must be {PROTOCOL_ID!r}"
                )
            _check_nonempty(
                _resolved_schema(documents, meta.get("properties", {}).get("idempotencyKey")),
                "schema request.meta.idempotencyKey", errors,
            )

    response = schemas.get("response")
    if response:
        _check_nonempty(_resolved_schema(documents, response.get("properties", {}).get("id")), "schema response.id", errors)
        _check_arbitrary(response.get("properties", {}).get("result"), "schema response.result", errors)

    event = schemas.get("event")
    if event:
        properties = event.get("properties", {})
        for name in ("sessionId", "streamId", "cursor", "eventId", "kind"):
            _check_nonempty(_resolved_schema(documents, properties.get(name)), f"schema event.{name}", errors)
        sequence = properties.get("sequence")
        if not isinstance(sequence, dict) or sequence.get("type") != "integer" or sequence.get("minimum") != 0:
            errors.append("schema event.sequence: must be an integer with minimum 0")
        if "requestId" in properties:
            _check_nonempty(_resolved_schema(documents, properties.get("requestId")), "schema event.requestId", errors)
        _check_arbitrary(properties.get("payload"), "schema event.payload", errors)

    error = schemas.get("error")
    if error:
        properties = error.get("properties", {})
        if "id" in properties:
            _check_nonempty(_resolved_schema(documents, properties.get("id")), "schema error.id", errors)
        payload = _resolved_schema(documents, properties.get("error"))
        _check_exact_object(
            payload, label="schema error.error", required=ERROR_REQUIRED, errors=errors
        )
        if isinstance(payload, dict):
            nested = payload.get("properties", {})
            _check_arbitrary(nested.get("details"), "schema error.error.details", errors)
            retryable = nested.get("retryable")
            if not isinstance(retryable, dict) or retryable.get("type") != "boolean":
                errors.append("schema error.error.retryable: must be boolean")

    replay_marker = _schema_with_property_const(documents, "method", "session.replay")
    if replay_marker is None:
        errors.append("schema: no request specialization with method const 'session.replay'")
    params = _object_schema_with_properties(documents, set(REPLAY_REQUIRED) | set(REPLAY_OPTIONAL))
    if params is None:
        errors.append("schema: no session.replay params object with sessionId/cursor/limit")
    else:
        _check_exact_object(
            params, label="schema session.replay params", required=REPLAY_REQUIRED,
            optional=REPLAY_OPTIONAL, errors=errors,
        )
        _check_nonempty(
            _resolved_schema(documents, params.get("properties", {}).get("sessionId")),
            "schema replay.sessionId", errors,
        )
        limit = _resolved_schema(documents, params.get("properties", {}).get("limit"))
        if not isinstance(limit, dict) or (
            limit.get("type") != "integer" or limit.get("minimum", 0) < 0
        ):
            errors.append("schema replay.limit: must be a nonnegative integer")
    return schemas


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


def check(root: Path) -> tuple[list[str], dict[str, Any]]:
    root = root.resolve()
    errors: list[str] = []
    documents = _load_documents(root / "protocol" / "schema" / "v1", errors)
    schemas = _check_schema_contract(documents, errors) if documents else {}
    if documents:
        _check_golden(documents, schemas, errors)
    groups = _check_sdks(root, errors)
    manifest = _manifest(root, documents, groups)
    return sorted(set(errors)), manifest


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
