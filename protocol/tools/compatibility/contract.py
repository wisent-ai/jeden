"""The contract this checker holds the schema to, written out on purpose.

Every other reader of the protocol takes its field names from the schema
document itself, because a name written twice drifts. A gate is the exception:
one that read its expectation out of the document under test would pass no
matter what that document said.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

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
