import assert from "node:assert/strict";
import test from "node:test";
import { omittedMetadata, publicMetadata, RedactingLogger, sensitiveMetadata } from "../src/logging.js";

class RecordingSink {
  readonly lines: string[] = [];
  appendLine(value: string): void { this.lines.push(value); }
}

function loggedPayload(line: string): Record<string, unknown> {
  const start = line.indexOf("{");
  assert.notEqual(start, -1);
  return JSON.parse(line.slice(start)) as Record<string, unknown>;
}

test("RedactingLogger removes credentials and user-controlled content before it reaches the sink", () => {
  const sink = new RecordingSink();
  const logger = new RedactingLogger(sink, () => true);
  const secret = "never-emit-this-secret";

  logger.event("acp.send", {
    method: publicMetadata("session/prompt"),
    id: publicMetadata(41),
    authorization: sensitiveMetadata(),
    cookie: sensitiveMetadata(),
    credential: sensitiveMetadata(),
    password: sensitiveMetadata(),
    prompt: sensitiveMetadata(),
    content: sensitiveMetadata(),
    text: sensitiveMetadata(),
    token: sensitiveMetadata(),
    path: sensitiveMetadata(),
    cwd: sensitiveMetadata(),
    uri: sensitiveMetadata(),
    input: sensitiveMetadata(),
    output: sensitiveMetadata(),
    detail: sensitiveMetadata(),
    nested: omittedMetadata(),
  });

  assert.equal(sink.lines.length, 1);
  assert.equal(sink.lines[0]?.includes(secret), false);
  assert.deepEqual(loggedPayload(sink.lines[0] ?? ""), {
    method: "session/prompt",
    id: 41,
    authorization: "[REDACTED]",
    cookie: "[REDACTED]",
    credential: "[REDACTED]",
    password: "[REDACTED]",
    prompt: "[REDACTED]",
    content: "[REDACTED]",
    text: "[REDACTED]",
    token: "[REDACTED]",
    path: "[REDACTED]",
    cwd: "[REDACTED]",
    uri: "[REDACTED]",
    input: "[REDACTED]",
    output: "[REDACTED]",
    detail: "[REDACTED]",
    nested: "[OMITTED]",
  });
});

test("RedactingLogger emits bounded metadata and only an error type", () => {
  const sink = new RecordingSink();
  const logger = new RedactingLogger(sink, () => true);
  logger.event("acp.metadata", { note: publicMetadata("x".repeat(200)), count: publicMetadata(3), active: publicMetadata(true), absent: publicMetadata(null) });
  const privateMessage = "message must not be logged";
  logger.error("acp.failure", new TypeError(privateMessage));

  assert.deepEqual(loggedPayload(sink.lines[0] ?? ""), { note: "x".repeat(120), count: 3, active: true, absent: null });
  assert.deepEqual(loggedPayload(sink.lines[1] ?? ""), { errorType: "TypeError" });
  assert.equal(sink.lines.join("\n").includes(privateMessage), false);
});
