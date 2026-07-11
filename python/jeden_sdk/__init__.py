"""Python SDK for the canonical ``jeden.session.v1`` envelope protocol."""

from .client import EnvelopeTransport, ProtocolError, SessionClient
from .models import (
    PROTOCOL_VERSION,
    Envelope,
    ErrorData,
    ErrorEnvelope,
    EventEnvelope,
    JsonObject,
    JsonValue,
    RequestEnvelope,
    RequestMeta,
    ResponseEnvelope,
    parse_envelope,
    validate_json,
)

__all__ = [
    "PROTOCOL_VERSION",
    "Envelope",
    "EnvelopeTransport",
    "ErrorData",
    "ErrorEnvelope",
    "EventEnvelope",
    "JsonObject",
    "JsonValue",
    "ProtocolError",
    "RequestEnvelope",
    "RequestMeta",
    "ResponseEnvelope",
    "SessionClient",
    "parse_envelope",
    "validate_json",
]
