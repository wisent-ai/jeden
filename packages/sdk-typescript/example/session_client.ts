import { SessionClient, type AsyncEnvelopeTransport, type RequestEnvelope } from "../src/index.js";

/** Deterministic injected service for this runnable example; production injects network I/O. */
class ExampleSessionService implements AsyncEnvelopeTransport {
  readonly #messages: unknown[] = [];
  readonly #waiters: Array<(result: IteratorResult<unknown>) => void> = [];

  async send(request: RequestEnvelope): Promise<void> {
    const response = request.method === "session.replay"
      ? { type: "response", id: request.id, result: { events: [], cursor: request.params } }
      : { type: "response", id: request.id, result: { sessionId: "example-session" } };
    const waiter = this.#waiters.shift();
    if (waiter === undefined) this.#messages.push(response);
    else waiter({ done: false, value: response });
  }

  async *receive(): AsyncIterable<unknown> {
    for (;;) {
      const message = this.#messages.shift();
      if (message !== undefined) { yield message; continue; }
      const { promise, resolve } = Promise.withResolvers<IteratorResult<unknown>>();
      this.#waiters.push(resolve);
      const next = await promise;
      if (next.done) return;
      yield next.value;
    }
  }
}

const client = new SessionClient(new ExampleSessionService());
const replay = await client.replay(
  { sessionId: "example-session", cursor: "cursor-10", limit: 20 },
  { idempotencyKey: "example-replay-1" },
);
console.log(JSON.stringify(replay));
await client.close();
