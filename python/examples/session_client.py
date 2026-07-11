"""Run with: PYTHONPATH=python python3 python/examples/session_client.py"""

import asyncio
from collections.abc import Mapping

from jeden_sdk import JsonObject, SessionClient


class ExampleTransport:
    def __init__(self) -> None:
        self.incoming: asyncio.Queue[dict[str, object]] = asyncio.Queue()

    async def send(self, envelope: JsonObject) -> None:
        await self.incoming.put(
            {"type": "response", "id": envelope["id"], "result": {"accepted": True}}
        )

    async def receive(self) -> Mapping[str, object]:
        return await self.incoming.get()


async def main() -> None:
    async with SessionClient(ExampleTransport()) as client:
        result = await client.request(
            "session.send",
            {"text": "Hello from Python"},
            request_id="example-request",
            idempotency_key="example-send-1",
        )
        print(result)


if __name__ == "__main__":
    asyncio.run(main())
