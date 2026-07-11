import assert from "node:assert/strict";
import test from "node:test";
import { ExtensionModel } from "../src/model.js";
import type { JsonValue, SessionEventEnvelope } from "../src/protocol.js";

function event(sequence: number, kind: string, payload: JsonValue): SessionEventEnvelope {
  return {
    type: "event",
    sessionId: "session-1",
    streamId: "stream-1",
    sequence,
    cursor: String(sequence),
    eventId: `event-${sequence}`,
    kind,
    payload,
  };
}

test("ExtensionModel projects streamed session events into consumer-visible views", () => {
  const model = new ExtensionModel();
  const changes: SessionEventEnvelope[] = [];
  model.on("change", (changed?: SessionEventEnvelope) => { if (changed) changes.push(changed); });

  const updates: readonly SessionEventEnvelope[] = [
    event(1, "agent_message_content", { text: "Implemented the change" }),
    event(2, "agent_thought", { content: "Checking edge cases" }),
    event(3, "tool_call", { title: "Run focused tests" }),
    event(4, "plan_update", { text: "Verify the result" }),
    event(5, "artifact_created", { id: "artifact-1", name: "report.json", uri: "file:///report.json", mediaType: "application/json" }),
    event(6, "worker_job_update", { jobId: "job-1", label: "Indexer", state: "running", detail: "2/4 shards" }),
    event(7, "diagnostic_update", { id: "diagnostic-1", uri: "file:///src/a.ts", message: "Type mismatch", severity: "error", line: 8, character: 13 }),
    event(8, "pending_diff", { id: "diff-1", title: "Apply fix", uri: "file:///src/a.ts", before: "old", after: "new", detail: "one-line replacement" }),
    event(9, "model_status", { label: "gpt-test ready" }),
    event(10, "account_status", { status: "signed in" }),
    event(11, "service_status", { message: "Connected" }),
  ];
  for (const update of updates) model.apply(update);

  assert.deepEqual(model.transcript, [
    { role: "agent", text: "Implemented the change" },
    { role: "status", text: "Checking edge cases" },
    { role: "tool", text: "Run focused tests" },
    { role: "plan", text: "Verify the result" },
  ]);
  assert.deepEqual(model.artifacts.get("artifact-1"), { id: "artifact-1", name: "report.json", uri: "file:///report.json", mediaType: "application/json" });
  assert.deepEqual(model.jobs.get("job-1"), { id: "job-1", label: "Indexer", state: "running", detail: "2/4 shards" });
  assert.deepEqual(model.diagnostics.get("diagnostic-1"), { id: "diagnostic-1", uri: "file:///src/a.ts", message: "Type mismatch", severity: "error", line: 8, character: 13 });
  assert.deepEqual(model.pending.get("diff-1"), { id: "diff-1", kind: "diff", title: "Apply fix", uri: "file:///src/a.ts", before: "old", after: "new", detail: "one-line replacement" });
  assert.equal(model.modelStatus, "gpt-test ready");
  assert.equal(model.accountStatus, "signed in");
  assert.equal(model.serviceStatus, "Connected");
  assert.deepEqual(changes, updates);
});

test("ExtensionModel renders ACP v1 updates serialized by the Rust agent", () => {
  const model = new ExtensionModel();
  const serializedUpdates: readonly SessionEventEnvelope[] = [
    event(1, "agent_message_chunk", { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Answer chunk" } }),
    event(2, "agent_thought_chunk", { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "Reasoning chunk" } }),
    event(3, "tool_call", { sessionUpdate: "tool_call", toolCallId: "tool-7", title: "Read workspace", status: "in_progress", rawInput: { path: "/workspace/a.ts" } }),
    event(4, "tool_call_update", { sessionUpdate: "tool_call_update", toolCallId: "tool-7", status: "completed", rawOutput: { bytes: 128 } }),
    event(5, "plan", {
      sessionUpdate: "plan",
      entries: [
        { content: "Implement client", priority: "high", status: "in_progress" },
        { content: "Verify integration", priority: "medium", status: "pending" },
      ],
    }),
  ];
  for (const update of serializedUpdates) model.apply(update);

  assert.deepEqual(model.transcript, [
    { role: "agent", text: "Answer chunk" },
    { role: "status", text: "Reasoning chunk" },
    { role: "tool", text: "Read workspace" },
    { role: "tool", text: "completed" },
    { role: "plan", text: "Implement client\nVerify integration" },
  ]);
});

test("ExtensionModel rejects incomplete artifacts, diagnostics, and diffs instead of exposing unusable records", () => {
  const model = new ExtensionModel();
  model.apply(event(1, "artifact_created", { id: "artifact-without-uri", name: "missing" }));
  model.apply(event(2, "diagnostic_update", { id: "diagnostic-without-message", uri: "file:///a.ts" }));
  model.apply(event(3, "pending_diff", { id: "diff-without-after", uri: "file:///a.ts", before: "old" }));

  assert.equal(model.artifacts.size, 0);
  assert.equal(model.diagnostics.size, 0);
  assert.equal(model.pending.size, 0);
});

test("clearSession removes session-scoped projections and emits one refresh", () => {
  const model = new ExtensionModel();
  model.apply(event(1, "agent_message_content", { text: "old turn" }));
  model.apply(event(2, "artifact_created", { id: "a", uri: "file:///a" }));
  model.apply(event(3, "worker_job_update", { id: "j", state: "done" }));
  model.apply(event(4, "diagnostic_update", { id: "d", uri: "file:///a", message: "broken" }));
  model.apply(event(5, "pending_diff", { id: "p", uri: "file:///a", before: "a", after: "b" }));
  let refreshes = 0;
  model.on("change", () => { refreshes += 1; });

  model.clearSession();

  assert.deepEqual({ transcript: model.transcript.length, artifacts: model.artifacts.size, jobs: model.jobs.size, diagnostics: model.diagnostics.size, pending: model.pending.size }, {
    transcript: 0,
    artifacts: 0,
    jobs: 0,
    diagnostics: 0,
    pending: 0,
  });
  assert.equal(refreshes, 1);
});
