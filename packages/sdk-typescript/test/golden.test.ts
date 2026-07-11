import goldenFixture from "./fixtures/envelopes.golden.json" with { type: "json" };
import {
  EnvelopeValidationError,
  PROTOCOL_VERSION,
  ProtocolError,
  SessionClient,
  isEnvelope,
  parseEnvelope,
  type AsyncEnvelopeTransport,
  type EventEnvelope,
  type JsonValue,
  type RequestEnvelope,
} from "../src/index.js";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (!Object.is(actual, expected)) throw new Error(`${message}: expected ${String(expected)}, received ${String(actual)}`);
}

async function assertRejects(operation: Promise<unknown>, check: (error: unknown) => boolean, message: string): Promise<void> {
  try {
    await operation;
  } catch (error) {
    assert(check(error), `${message}: wrong error ${String(error)}`);
    return;
  }
  throw new Error(`${message}: promise resolved`);
}

interface QueueWaiter { resolve(result: IteratorResult<unknown>): void; }

class DeterministicTransport implements AsyncEnvelopeTransport {
  readonly sent: RequestEnvelope[] = [];
  readonly #incoming: unknown[] = [];
  readonly #waiters: QueueWaiter[] = [];
  #closed = false;

  async send(envelope: RequestEnvelope): Promise<void> {
    this.sent.push(envelope);
    if (envelope.method === "session.fail") {
      this.push({ type: "error", id: envelope.id, error: { code: "session.failed", message: "deterministic failure", retryable: false, details: null } });
      return;
    }
    if (envelope.method === "session.mutate") {
      this.push({
        type: "event", sessionId: "session-001", streamId: "stream-001",
        sequence: 9007199254740991, cursor: "cursor-preserved", eventId: "event-001",
        requestId: envelope.id, kind: "session.changed", payload: { applied: true },
      } satisfies EventEnvelope);
    }
    this.push({ type: "response", id: envelope.id, result: { method: envelope.method, params: envelope.params } });
  }

  receive(): AsyncIterable<unknown> { return this; }

  [Symbol.asyncIterator](): AsyncIterator<unknown> {
    return { next: () => {
      const value = this.#incoming.shift();
      if (value !== undefined) return Promise.resolve({ done: false, value });
      if (this.#closed) return Promise.resolve({ done: true, value: undefined });
      const { promise, resolve } = Promise.withResolvers<IteratorResult<unknown>>();
      this.#waiters.push({ resolve });
      return promise;
    } };
  }

  async close(): Promise<void> {
    this.#closed = true;
    for (const waiter of this.#waiters.splice(0)) waiter.resolve({ done: true, value: undefined });
  }

  private push(value: unknown): void {
    const waiter = this.#waiters.shift();
    if (waiter !== undefined) waiter.resolve({ done: false, value });
    else this.#incoming.push(value);
  }
}

function testGoldenRoundTrips(): void {
  const fixture: unknown = goldenFixture;
  assert(Array.isArray(fixture), "golden fixture must be an array");
  assertEqual(fixture.length, 4, "golden fixture must contain all four envelope variants");
  const parsed = fixture.map((value) => parseEnvelope(value));
  assertEqual(parsed.map(({ type }) => type).join(","), "request,response,event,error", "variant order");
  for (const envelope of parsed) {
    const roundTripped: unknown = JSON.parse(JSON.stringify(envelope));
    assert(isEnvelope(roundTripped), `roundtrip rejected ${envelope.type}`);
    assertEqual(JSON.stringify(roundTripped), JSON.stringify(envelope), `roundtrip changed ${envelope.type}`);
  }
}

function testStrictValidators(): void {
  const valid = goldenFixture as unknown[];
  const request = structuredClone(valid[0]) as Record<string, unknown>;
  request.extra = true;
  assert(!isEnvelope(request), "unknown request field must be rejected");
  const nestedMeta = structuredClone(valid[0]) as { meta: Record<string, unknown> };
  nestedMeta.meta.extra = true;
  assert(!isEnvelope(nestedMeta), "unknown meta field must be rejected");
  const nestedError = structuredClone(valid[3]) as { error: Record<string, unknown> };
  nestedError.error.extra = true;
  assert(!isEnvelope(nestedError), "unknown error field must be rejected");
  const malformedEvent = structuredClone(valid[2]) as { sequence: number };
  malformedEvent.sequence = -1;
  assert(!isEnvelope(malformedEvent), "negative sequence must be rejected");
  malformedEvent.sequence = 1.5;
  assert(!isEnvelope(malformedEvent), "fractional sequence must be rejected");
  const missingResult = structuredClone(valid[1]) as Record<string, unknown>;
  delete missingResult.result;
  assert(!isEnvelope(missingResult), "missing result must be rejected");
  let validationError: unknown;
  try { parseEnvelope({ type: "event" }); } catch (error) { validationError = error; }
  assert(validationError instanceof EnvelopeValidationError, "parser must throw typed validation error");
}

async function testClient(): Promise<void> {
  const transport = new DeterministicTransport();
  const client = new SessionClient(transport);
  const eventPromise = client.events().next();
  const result = await client.request<{ method: string; params: JsonValue }>(
    "session.mutate", { value: 42 },
    { id: "mutation-001", idempotencyKey: "caller-key-001", traceId: "trace-client" },
  );
  assertEqual(result.method, "session.mutate", "response correlation");
  const event = await eventPromise;
  assert(!event.done, "event stream ended unexpectedly");
  assertEqual(event.value.sequence, 9007199254740991, "sequence must be preserved");
  assertEqual(event.value.cursor, "cursor-preserved", "cursor must be preserved");
  assertEqual(event.value.requestId, "mutation-001", "event request correlation");

  const replay = await client.replay<{ method: string; params: JsonValue }>(
    { sessionId: "session-001", cursor: "cursor-preserved", limit: 5 },
    { idempotencyKey: "caller-key-replay" },
  );
  assertEqual(replay.method, "session.replay", "replay method");
  const replayRequest = transport.sent[1];
  assert(replayRequest !== undefined, "replay request missing");
  assertEqual(replayRequest.meta.protocolVersion, PROTOCOL_VERSION, "protocol version");
  assertEqual(replayRequest.meta.idempotencyKey, "caller-key-replay", "caller idempotency key");
  assertEqual(JSON.stringify(replayRequest.params), JSON.stringify({ sessionId: "session-001", cursor: "cursor-preserved", limit: 5 }), "replay params");

  await assertRejects(
    client.request("session.fail", null, { id: "failure-001", idempotencyKey: "caller-key-failure" }),
    (error) => error instanceof ProtocolError && error.code === "session.failed" && error.requestId === "failure-001" && !error.retryable,
    "server error must become ProtocolError",
  );
  const unsafeClient = client as unknown as { request(method: string, params: JsonValue, options: Record<string, never>): Promise<JsonValue> };
  await assertRejects(
    unsafeClient.request("session.mutate", null, {}),
    (error) => error instanceof TypeError && error.message === "idempotencyKey must be a non-empty string",
    "runtime callers must supply idempotency key",
  );
  await client.close();
}

async function main(): Promise<void> {
  testGoldenRoundTrips();
  testStrictValidators();
  await testClient();
  console.log("sdk-typescript golden and client tests passed");
}

await main();
