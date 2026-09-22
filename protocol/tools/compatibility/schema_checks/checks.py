"""What the schema has to say: the exact objects each envelope kind declares,
the fields that must be present, and the shapes the meta and error members
take. This is the part that fails when the protocol changes without its
checker being told."""

from __future__ import annotations

from typing import Any, Sequence

from ..contract import (
    CAMEL_FIELDS,
    ERROR_REQUIRED,
    KINDS,
    META_OPTIONAL,
    META_REQUIRED,
    PROTOCOL_ID,
    REPLAY_OPTIONAL,
    REPLAY_REQUIRED,
    REQUIRED_FIELDS,
    Document,
)
from ..documents import (
    _object_schema_for_kind,
    _object_schema_with_properties,
    _resolved_schema,
    _schema_with_property_const,
)

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


