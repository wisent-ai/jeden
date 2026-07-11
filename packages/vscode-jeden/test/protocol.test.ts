import assert from "node:assert/strict";
import test from "node:test";
import { parseMessage } from "../src/protocol.js";

test("parseMessage preserves JSON-RPC requests, notifications, results, and agent errors", () => {
  assert.deepEqual(parseMessage({ jsonrpc: "2.0", id: 7, method: "session/prompt", params: { prompt: ["hello"] } }), {
    jsonrpc: "2.0",
    id: 7,
    method: "session/prompt",
    params: { prompt: ["hello"] },
  });
  assert.deepEqual(parseMessage({ jsonrpc: "2.0", method: "session/update" }), {
    jsonrpc: "2.0",
    method: "session/update",
    params: {},
  });
  assert.deepEqual(parseMessage({ jsonrpc: "2.0", id: 7, result: { stopReason: "end_turn" } }), {
    jsonrpc: "2.0",
    id: 7,
    result: { stopReason: "end_turn" },
  });
  assert.deepEqual(parseMessage({ jsonrpc: "2.0", id: 7, error: { code: -32000, message: "agent failed", data: { retryable: false } } }), {
    jsonrpc: "2.0",
    id: 7,
    error: { code: -32000, message: "agent failed", data: { retryable: false } },
  });
});

test("parseMessage rejects malformed envelopes and values that cannot cross JSON-RPC", () => {
  const invalid: readonly unknown[] = [
    null,
    { jsonrpc: "1.0", id: 1, result: null },
    { jsonrpc: "2.0", id: 1.5, result: null },
    { jsonrpc: "2.0", id: 1, method: 42, params: {} },
    { jsonrpc: "2.0", id: 1, result: Number.POSITIVE_INFINITY },
    { jsonrpc: "2.0", id: 1, result: { unsupported: undefined } },
  ];
  for (const value of invalid) assert.throws(() => parseMessage(value), Error);
});
