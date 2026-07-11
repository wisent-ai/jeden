"""Asynchronous client using an application-injected envelope transport."""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator, Mapping
from typing import Protocol, runtime_checkable
from uuid import uuid4

from .models import (
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


@runtime_checkable
class EnvelopeTransport(Protocol):
    """Minimal bidirectional transport; framing and I/O belong to the injector."""

    async def send(self, envelope: JsonObject) -> None: ...

    async def receive(self) -> Mapping[str, object]: ...


class ProtocolError(Exception):
    """A correlated ``error`` envelope returned by the protocol peer."""

    def __init__(self, envelope: ErrorEnvelope) -> None:
        self.envelope = envelope
        self.id = envelope.id
        self.code = envelope.error.code
        self.message = envelope.error.message
        self.retryable = envelope.error.retryable
        self.details = envelope.error.details
        super().__init__(f"{self.code}: {self.message}")


class SessionClient:
    """Correlates requests while exposing the ordered event stream separately."""

    def __init__(self, transport: EnvelopeTransport) -> None:
        self._transport = transport
        self._pending: dict[str, asyncio.Future[JsonValue]] = {}
        self._events: asyncio.Queue[EventEnvelope | BaseException] = asyncio.Queue()
        self._reader_task: asyncio.Task[None] | None = None
        self._closed = False

    async def request(
        self,
        method: str,
        params: JsonValue,
        *,
        idempotency_key: str | None = None,
        deadline: str | None = None,
        trace_id: str | None = None,
        request_id: str | None = None,
    ) -> JsonValue:
        if self._closed:
            raise RuntimeError("SessionClient is closed")
        if not isinstance(method, str) or not method:
            raise ValueError("method must be a non-empty string")
        params = validate_json(params, "params")
        identifier = request_id or str(uuid4())
        if not isinstance(identifier, str) or not identifier:
            raise ValueError("request_id must be a non-empty string")
        if method != "session.replay" and not idempotency_key:
            raise ValueError("mutating requests require a caller-supplied idempotency_key")
        if idempotency_key is not None and (not isinstance(idempotency_key, str) or not idempotency_key):
            raise ValueError("idempotency_key must be a non-empty string")
        if identifier in self._pending:
            raise ValueError(f"request id is already pending: {identifier}")

        meta = RequestMeta(
            idempotency_key=idempotency_key or f"read:{identifier}",
            deadline=deadline,
            trace_id=trace_id,
        )
        # Round-trip through validation so direct dataclass construction cannot emit
        # invalid optional metadata.
        envelope = RequestEnvelope.from_dict(
            {"type": "request", "id": identifier, "method": method, "params": params, "meta": meta.to_dict()}
        )
        loop = asyncio.get_running_loop()
        future: asyncio.Future[JsonValue] = loop.create_future()
        self._pending[identifier] = future
        self._ensure_reader()
        try:
            await self._transport.send(envelope.to_dict())
        except BaseException:
            self._pending.pop(identifier, None)
            future.cancel()
            raise
        try:
            return await future
        except asyncio.CancelledError:
            self._pending.pop(identifier, None)
            raise

    async def replay(
        self,
        session_id: str,
        *,
        cursor: str | None = None,
        limit: int | None = None,
        request_id: str | None = None,
        trace_id: str | None = None,
    ) -> JsonValue:
        if not isinstance(session_id, str) or not session_id:
            raise ValueError("session_id must be a non-empty string")
        if cursor is not None and (not isinstance(cursor, str) or not cursor):
            raise ValueError("cursor must be a non-empty string")
        if limit is not None and (isinstance(limit, bool) or not isinstance(limit, int) or limit < 1):
            raise ValueError("limit must be a positive integer")
        params: JsonObject = {"sessionId": session_id}
        if cursor is not None:
            params["cursor"] = cursor
        if limit is not None:
            params["limit"] = limit
        return await self.request(
            "session.replay",
            params,
            request_id=request_id,
            trace_id=trace_id,
        )

    async def events(self) -> AsyncIterator[EventEnvelope]:
        """Yield validated events in the exact order received from the transport."""
        if self._closed:
            raise RuntimeError("SessionClient is closed")
        self._ensure_reader()
        while True:
            item = await self._events.get()
            if isinstance(item, BaseException):
                raise item
            yield item

    async def aclose(self) -> None:
        if self._closed:
            return
        self._closed = True
        task = self._reader_task
        if task is not None:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass
        failure = RuntimeError("SessionClient is closed")
        self._fail_pending(failure)

    async def __aenter__(self) -> "SessionClient":
        return self

    async def __aexit__(self, exc_type: object, exc: object, traceback: object) -> None:
        await self.aclose()

    def _ensure_reader(self) -> None:
        if self._reader_task is None:
            self._reader_task = asyncio.create_task(self._reader_loop())

    async def _reader_loop(self) -> None:
        try:
            while True:
                raw = await self._transport.receive()
                envelope = parse_envelope(raw)
                if isinstance(envelope, EventEnvelope):
                    await self._events.put(envelope)
                elif isinstance(envelope, ResponseEnvelope):
                    future = self._pending.pop(envelope.id, None)
                    if future is None:
                        raise ValueError(f"response has no pending request: {envelope.id}")
                    future.set_result(envelope.result)
                elif isinstance(envelope, ErrorEnvelope):
                    error = ProtocolError(envelope)
                    if envelope.id is None:
                        raise error
                    future = self._pending.pop(envelope.id, None)
                    if future is None:
                        raise ValueError(f"error has no pending request: {envelope.id}")
                    future.set_exception(error)
                else:
                    raise ValueError("client transport received a request envelope")
        except asyncio.CancelledError:
            raise
        except BaseException as error:
            self._closed = True
            self._fail_pending(error)
            await self._events.put(error)

    def _fail_pending(self, error: BaseException) -> None:
        pending, self._pending = self._pending, {}
        for future in pending.values():
            if not future.done():
                future.set_exception(error)
