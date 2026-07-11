from __future__ import annotations

import asyncio
import json
from collections.abc import Mapping
from copy import deepcopy
from pathlib import Path
import sys
import unittest

PYTHON_ROOT = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = PYTHON_ROOT.parent
sys.path.insert(0, str(PYTHON_ROOT))

from jeden_sdk import (  # noqa: E402
    EventEnvelope,
    JsonObject,
    ProtocolError,
    SessionClient,
    parse_envelope,
)

GOLDEN_PATH = REPOSITORY_ROOT / "protocol" / "schema" / "v1" / "golden" / "envelopes.json"


class DeterministicTransport:
    def __init__(self) -> None:
        self.sent: list[JsonObject] = []
        self.incoming: asyncio.Queue[Mapping[str, object]] = asyncio.Queue()
        self.sent_event = asyncio.Event()
        self.receiving_event = asyncio.Event()

    async def send(self, envelope: JsonObject) -> None:
        self.sent.append(envelope)
        self.sent_event.set()

    async def receive(self) -> Mapping[str, object]:
        self.receiving_event.set()
        return await self.incoming.get()


class EnvelopeValidationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.golden: list[dict[str, object]] = json.loads(GOLDEN_PATH.read_text(encoding="utf-8"))

    def test_all_four_golden_envelopes_round_trip_exactly(self) -> None:
        self.assertEqual(["request", "response", "event", "error"], [item["type"] for item in self.golden])
        for raw in self.golden:
            with self.subTest(envelope_type=raw["type"]):
                self.assertEqual(raw, parse_envelope(raw).to_dict())

    def test_unknown_envelope_meta_and_error_fields_are_rejected(self) -> None:
        cases: list[dict[str, object]] = []
        request = deepcopy(self.golden[0])
        request["extra"] = True
        cases.append(request)
        meta = deepcopy(self.golden[0])
        meta["meta"]["extra"] = True  # type: ignore[index]
        cases.append(meta)
        error = deepcopy(self.golden[3])
        error["error"]["extra"] = True  # type: ignore[index]
        cases.append(error)
        for raw in cases:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                parse_envelope(raw)

    def test_malformed_required_fields_and_json_values_are_rejected(self) -> None:
        malformed = [
            {**self.golden[2], "sequence": -1},
            {**self.golden[2], "sequence": True},
            {**self.golden[2], "cursor": ""},
            {**self.golden[1], "id": ""},
            {**self.golden[0], "params": {"sessionId": "s", "limit": 0}},
            {**self.golden[0], "params": {"sessionId": "s", "unexpected": 1}},
            {**self.golden[1], "result": object()},
        ]
        for raw in malformed:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                parse_envelope(raw)


class SessionClientTests(unittest.IsolatedAsyncioTestCase):
    async def test_replay_preserves_params_and_correlates_response(self) -> None:
        transport = DeterministicTransport()
        client = SessionClient(transport)
        replay = asyncio.create_task(
            client.replay("session-1", cursor="cursor-41", limit=100, request_id="replay-1", trace_id="trace-1")
        )
        await transport.sent_event.wait()
        self.assertEqual("session.replay", transport.sent[0]["method"])
        self.assertEqual(
            {"sessionId": "session-1", "cursor": "cursor-41", "limit": 100},
            transport.sent[0]["params"],
        )
        self.assertEqual("jeden.session.v1", transport.sent[0]["meta"]["protocolVersion"])  # type: ignore[index]
        await transport.incoming.put({"type": "response", "id": "replay-1", "result": {"count": 3}})
        self.assertEqual({"count": 3}, await replay)
        await client.aclose()

    async def test_error_is_correlated_as_protocol_error(self) -> None:
        transport = DeterministicTransport()
        client = SessionClient(transport)
        request = asyncio.create_task(
            client.request(
                "session.send",
                {"text": "hello"},
                idempotency_key="send-once",
                request_id="send-1",
            )
        )
        await transport.sent_event.wait()
        await transport.incoming.put(
            {
                "type": "error",
                "id": "send-1",
                "error": {
                    "code": "session.busy",
                    "message": "busy",
                    "retryable": True,
                    "details": {"retryAfterMs": 10},
                },
            }
        )
        with self.assertRaises(ProtocolError) as raised:
            await request
        self.assertEqual("send-1", raised.exception.id)
        self.assertEqual("session.busy", raised.exception.code)
        self.assertTrue(raised.exception.retryable)
        self.assertEqual({"retryAfterMs": 10}, raised.exception.details)
        await client.aclose()

    async def test_events_preserve_sequence_cursor_and_correlation(self) -> None:
        transport = DeterministicTransport()
        client = SessionClient(transport)
        events = client.events()
        next_event = asyncio.create_task(anext(events))
        await transport.receiving_event.wait()
        raw = {
            "type": "event",
            "sessionId": "session-1",
            "streamId": "stream-1",
            "sequence": 42,
            "cursor": "cursor-42",
            "eventId": "event-42",
            "requestId": "request-1",
            "kind": "session.output.delta",
            "payload": {"text": "hello"},
        }
        await transport.incoming.put(raw)
        event = await next_event
        self.assertIsInstance(event, EventEnvelope)
        self.assertEqual((42, "cursor-42", "request-1"), (event.sequence, event.cursor, event.request_id))
        await events.aclose()
        await client.aclose()

    async def test_mutating_request_requires_caller_idempotency_key(self) -> None:
        client = SessionClient(DeterministicTransport())
        with self.assertRaisesRegex(ValueError, "idempotency_key"):
            await client.request("session.send", {"text": "hello"}, request_id="send-1")
        await client.aclose()


if __name__ == "__main__":
    unittest.main()
