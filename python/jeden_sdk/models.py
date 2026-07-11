"""Exact wire models for the ``jeden.session.v1`` protocol."""

from __future__ import annotations

from dataclasses import dataclass
import math
from typing import Any, Mapping, TypeAlias

PROTOCOL_VERSION = "jeden.session.v1"
JsonValue: TypeAlias = None | bool | int | float | str | list["JsonValue"] | dict[str, "JsonValue"]
JsonObject: TypeAlias = dict[str, JsonValue]


def _object(value: object, where: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or any(not isinstance(key, str) for key in value):
        raise ValueError(f"{where} must be an object with string keys")
    return value


def _exact(value: Mapping[str, object], required: set[str], optional: set[str], where: str) -> None:
    missing = required - value.keys()
    unknown = value.keys() - required - optional
    if missing:
        raise ValueError(f"{where} is missing fields: {', '.join(sorted(missing))}")
    if unknown:
        raise ValueError(f"{where} has unknown fields: {', '.join(sorted(unknown))}")


def _nonempty(value: object, where: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{where} must be a non-empty string")
    return value


def validate_json(value: object, where: str = "value") -> JsonValue:
    if value is None or isinstance(value, (str, bool)):
        return value
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError(f"{where} must contain finite JSON numbers")
        return value
    if isinstance(value, list):
        return [validate_json(item, f"{where}[{index}]") for index, item in enumerate(value)]
    if isinstance(value, Mapping):
        if any(not isinstance(key, str) for key in value):
            raise ValueError(f"{where} must contain only string object keys")
        return {key: validate_json(item, f"{where}.{key}") for key, item in value.items()}
    raise ValueError(f"{where} must be a JSON value")


@dataclass(frozen=True, slots=True)
class RequestMeta:
    idempotency_key: str
    deadline: str | None = None
    trace_id: str | None = None
    protocol_version: str = PROTOCOL_VERSION

    @classmethod
    def from_dict(cls, raw: object) -> "RequestMeta":
        value = _object(raw, "request.meta")
        _exact(value, {"protocolVersion", "idempotencyKey"}, {"deadline", "traceId"}, "request.meta")
        protocol_version = _nonempty(value["protocolVersion"], "request.meta.protocolVersion")
        if protocol_version != PROTOCOL_VERSION:
            raise ValueError(f"request.meta.protocolVersion must be {PROTOCOL_VERSION!r}")
        deadline = _nonempty(value["deadline"], "request.meta.deadline") if "deadline" in value else None
        trace_id = _nonempty(value["traceId"], "request.meta.traceId") if "traceId" in value else None
        return cls(
            protocol_version=protocol_version,
            idempotency_key=_nonempty(value["idempotencyKey"], "request.meta.idempotencyKey"),
            deadline=deadline,
            trace_id=trace_id,
        )

    def to_dict(self) -> JsonObject:
        value: JsonObject = {
            "protocolVersion": self.protocol_version,
            "idempotencyKey": self.idempotency_key,
        }
        if self.deadline is not None:
            value["deadline"] = self.deadline
        if self.trace_id is not None:
            value["traceId"] = self.trace_id
        return value


def _request_params(method: str, raw: object) -> JsonValue:
    params = validate_json(raw, "request.params")
    if method != "session.replay":
        return params
    value = _object(params, "request.params")
    _exact(value, {"sessionId"}, {"cursor", "limit"}, "request.params")
    _nonempty(value["sessionId"], "request.params.sessionId")
    if "cursor" in value:
        _nonempty(value["cursor"], "request.params.cursor")
    if "limit" in value:
        limit = value["limit"]
        if isinstance(limit, bool) or not isinstance(limit, int) or limit < 1:
            raise ValueError("request.params.limit must be a positive integer")
    return params


@dataclass(frozen=True, slots=True)
class RequestEnvelope:
    id: str
    method: str
    params: JsonValue
    meta: RequestMeta
    type: str = "request"

    @classmethod
    def from_dict(cls, raw: object) -> "RequestEnvelope":
        value = _object(raw, "request")
        _exact(value, {"type", "id", "method", "params", "meta"}, set(), "request")
        if value["type"] != "request":
            raise ValueError("request.type must be 'request'")
        method = _nonempty(value["method"], "request.method")
        return cls(
            id=_nonempty(value["id"], "request.id"),
            method=method,
            params=_request_params(method, value["params"]),
            meta=RequestMeta.from_dict(value["meta"]),
        )

    def to_dict(self) -> JsonObject:
        return {"type": self.type, "id": self.id, "method": self.method, "params": self.params, "meta": self.meta.to_dict()}


@dataclass(frozen=True, slots=True)
class ResponseEnvelope:
    id: str
    result: JsonValue
    type: str = "response"

    @classmethod
    def from_dict(cls, raw: object) -> "ResponseEnvelope":
        value = _object(raw, "response")
        _exact(value, {"type", "id", "result"}, set(), "response")
        if value["type"] != "response":
            raise ValueError("response.type must be 'response'")
        return cls(id=_nonempty(value["id"], "response.id"), result=validate_json(value["result"], "response.result"))

    def to_dict(self) -> JsonObject:
        return {"type": self.type, "id": self.id, "result": self.result}


@dataclass(frozen=True, slots=True)
class EventEnvelope:
    session_id: str
    stream_id: str
    sequence: int
    cursor: str
    event_id: str
    kind: str
    payload: JsonValue
    request_id: str | None = None
    type: str = "event"

    @classmethod
    def from_dict(cls, raw: object) -> "EventEnvelope":
        value = _object(raw, "event")
        _exact(value, {"type", "sessionId", "streamId", "sequence", "cursor", "eventId", "kind", "payload"}, {"requestId"}, "event")
        if value["type"] != "event":
            raise ValueError("event.type must be 'event'")
        sequence = value["sequence"]
        if isinstance(sequence, bool) or not isinstance(sequence, int) or sequence < 0:
            raise ValueError("event.sequence must be a nonnegative integer")
        request_id = _nonempty(value["requestId"], "event.requestId") if "requestId" in value else None
        return cls(
            session_id=_nonempty(value["sessionId"], "event.sessionId"),
            stream_id=_nonempty(value["streamId"], "event.streamId"),
            sequence=sequence,
            cursor=_nonempty(value["cursor"], "event.cursor"),
            event_id=_nonempty(value["eventId"], "event.eventId"),
            request_id=request_id,
            kind=_nonempty(value["kind"], "event.kind"),
            payload=validate_json(value["payload"], "event.payload"),
        )

    def to_dict(self) -> JsonObject:
        value: JsonObject = {
            "type": self.type,
            "sessionId": self.session_id,
            "streamId": self.stream_id,
            "sequence": self.sequence,
            "cursor": self.cursor,
            "eventId": self.event_id,
            "kind": self.kind,
            "payload": self.payload,
        }
        if self.request_id is not None:
            value["requestId"] = self.request_id
        return value


@dataclass(frozen=True, slots=True)
class ErrorData:
    code: str
    message: str
    retryable: bool
    details: JsonValue

    @classmethod
    def from_dict(cls, raw: object) -> "ErrorData":
        value = _object(raw, "error.error")
        _exact(value, {"code", "message", "retryable", "details"}, set(), "error.error")
        if not isinstance(value["code"], str):
            raise ValueError("error.error.code must be a string")
        if not isinstance(value["message"], str):
            raise ValueError("error.error.message must be a string")
        if not isinstance(value["retryable"], bool):
            raise ValueError("error.error.retryable must be a boolean")
        return cls(
            code=value["code"],
            message=value["message"],
            retryable=value["retryable"],
            details=validate_json(value["details"], "error.error.details"),
        )

    def to_dict(self) -> JsonObject:
        return {"code": self.code, "message": self.message, "retryable": self.retryable, "details": self.details}


@dataclass(frozen=True, slots=True)
class ErrorEnvelope:
    error: ErrorData
    id: str | None = None
    type: str = "error"

    @classmethod
    def from_dict(cls, raw: object) -> "ErrorEnvelope":
        value = _object(raw, "error")
        _exact(value, {"type", "error"}, {"id"}, "error")
        if value["type"] != "error":
            raise ValueError("error.type must be 'error'")
        identifier = _nonempty(value["id"], "error.id") if "id" in value else None
        return cls(id=identifier, error=ErrorData.from_dict(value["error"]))

    def to_dict(self) -> JsonObject:
        value: JsonObject = {"type": self.type, "error": self.error.to_dict()}
        if self.id is not None:
            value["id"] = self.id
        return value


Envelope: TypeAlias = RequestEnvelope | ResponseEnvelope | EventEnvelope | ErrorEnvelope


def parse_envelope(raw: object) -> Envelope:
    value = _object(raw, "envelope")
    envelope_type = value.get("type")
    parsers = {
        "request": RequestEnvelope.from_dict,
        "response": ResponseEnvelope.from_dict,
        "event": EventEnvelope.from_dict,
        "error": ErrorEnvelope.from_dict,
    }
    parser = parsers.get(envelope_type) if isinstance(envelope_type, str) else None
    if parser is None:
        raise ValueError("envelope.type must be one of: request, response, event, error")
    return parser(value)
