import assert from "node:assert/strict";
import { EventEmitter, once } from "node:events";
import test from "node:test";
import { AcpError, JedenAcpClient, type ClientInteraction } from "../src/client.js";
import { RedactingLogger } from "../src/logging.js";
import type { JsonRpcMessage, JsonRpcRequest, JsonValue, SessionEventEnvelope } from "../src/protocol.js";
import type { AcpTransport } from "../src/transport.js";

const capabilities = {
  loadSession: true,
  promptCapabilities: { image: true, audio: false, embeddedContext: true },
  sessionCapabilities: { input: true, terminal: true },
} as const;

class MockAcpTransport implements AcpTransport {
  readonly events = new EventEmitter();
  readonly sent: JsonRpcMessage[] = [];
  started = false;
  closed = false;

  constructor(private readonly agent: (message: JsonRpcMessage, transport: MockAcpTransport) => void) {}

  async start(): Promise<void> { this.started = true; }
  async send(message: JsonRpcMessage): Promise<void> {
    if (!this.started || this.closed) throw new Error("mock transport is disconnected");
    this.sent.push(message);
    this.events.emit("sent", message);
    this.agent(message, this);
  }
  async close(): Promise<void> { this.closed = true; }
  receive(message: JsonRpcMessage): void { this.events.emit("message", message); }
  disconnect(intentional = false): void {
    this.closed = true;
    this.events.emit("close", { intentional, code: 1, signal: null });
  }
  reply(request: JsonRpcRequest, result: JsonValue): void {
    queueMicrotask(() => this.receive({ jsonrpc: "2.0", id: request.id, result }));
  }
}

function isRequest(message: JsonRpcMessage): message is JsonRpcRequest {
  return "method" in message && "id" in message;
}

function createClient(
  factory: () => MockAcpTransport,
  interaction: ClientInteraction = {
    async requestPermission() { return undefined; },
    async requestInput() { return undefined; },
  },
  reconnect: { readonly enabled: boolean; readonly limit: number } = { enabled: false, limit: 0 },
): JedenAcpClient {
  return new JedenAcpClient({
    transport: factory,
    interaction,
    logger: new RedactingLogger({ appendLine() {} }, () => false),
    autoReconnect: () => reconnect.enabled,
    reconnectLimit: () => reconnect.limit,
  });
}

function standardAgent(message: JsonRpcMessage, transport: MockAcpTransport): void {
  if (!isRequest(message)) return;
  if (message.method === "initialize") {
    transport.reply(message, { protocolVersion: 1, agentCapabilities: capabilities, agentInfo: { name: "mock-agent", version: "9.1" } });
  } else if (message.method === "session/new") {
    transport.reply(message, { sessionId: "session-new" });
  } else if (message.method === "session/load") {
    transport.reply(message, {});
  }
}

async function waitForSent(transport: MockAcpTransport, predicate: (message: JsonRpcMessage) => boolean): Promise<JsonRpcMessage> {
  const existing = transport.sent.find(predicate);
  if (existing) return existing;
  return await new Promise<JsonRpcMessage>((resolve) => {
    const listener = (message: JsonRpcMessage): void => {
      if (!predicate(message)) return;
      transport.events.off("sent", listener);
      resolve(message);
    };
    transport.events.on("sent", listener);
  });
}

test("client initializes, creates and loads sessions, prompts, and cancels an active turn", async (t) => {
  let promptRequest: JsonRpcRequest | undefined;
  const transport = new MockAcpTransport((message, activeTransport) => {
    standardAgent(message, activeTransport);
    if (isRequest(message) && message.method === "session/prompt") promptRequest = message;
  });
  const client = createClient(() => transport);
  t.after(async () => client.dispose());
  const turnStates: boolean[] = [];
  client.on("turn", (active: boolean) => turnStates.push(active));

  const initialized = await client.connect();
  assert.deepEqual(initialized, { protocolVersion: 1, agentCapabilities: capabilities, agentInfo: { name: "mock-agent", version: "9.1" } });
  assert.deepEqual(transport.sent[0], {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: 1,
      clientCapabilities: { elicitation: { form: {} } },
      clientInfo: { name: "vscode-jeden", version: "0.1.0" },
    },
  });

  assert.deepEqual(await client.newSession("/workspace/new"), { id: "session-new", cwd: "/workspace/new" });
  assert.deepEqual(await client.loadSession("session-existing", "/workspace/existing"), { id: "session-existing", cwd: "/workspace/existing" });

  const prompt = client.prompt("fix the failing test");
  const sentPrompt = await waitForSent(transport, (message) => isRequest(message) && message.method === "session/prompt");
  assert.deepEqual(sentPrompt, {
    jsonrpc: "2.0",
    id: 4,
    method: "session/prompt",
    params: { sessionId: "session-existing", prompt: [{ type: "text", text: "fix the failing test" }] },
  });
  assert.equal(client.turnActive, true);
  await client.cancel();
  assert.deepEqual(transport.sent.at(-1), { jsonrpc: "2.0", method: "session/cancel", params: { sessionId: "session-existing" } });

  assert.ok(promptRequest);
  transport.receive({ jsonrpc: "2.0", id: promptRequest.id, result: { stopReason: "end_turn" } });
  assert.deepEqual(await prompt, { stopReason: "end_turn" });
  assert.equal(client.turnActive, false);
  assert.deepEqual(turnStates, [true, false]);
});

test("client maps agent failures to AcpError and always closes the turn", async (t) => {
  const transport = new MockAcpTransport((message, activeTransport) => {
    standardAgent(message, activeTransport);
    if (isRequest(message) && message.method === "session/prompt") {
      queueMicrotask(() => activeTransport.receive({ jsonrpc: "2.0", id: message.id, error: { code: -32042, message: "model unavailable", data: { retryable: true } } }));
    }
  });
  const client = createClient(() => transport);
  t.after(async () => client.dispose());
  await client.connect();
  await client.newSession("/workspace");

  await assert.rejects(client.prompt("try once"), (error: unknown) => {
    assert.ok(error instanceof AcpError);
    assert.equal(error.code, -32042);
    assert.equal(error.message, "model unavailable");
    assert.deepEqual(error.data, { retryable: true });
    return true;
  });
  assert.equal(client.turnActive, false);
});

test("client emits canonical updates unchanged and assigns stable ordering to legacy updates", async (t) => {
  const transport = new MockAcpTransport(standardAgent);
  const client = createClient(() => transport);
  t.after(async () => client.dispose());
  await client.connect();
  const events: SessionEventEnvelope[] = [];
  client.on("event", (event: SessionEventEnvelope) => events.push(event));

  const canonical: SessionEventEnvelope = {
    type: "event",
    sessionId: "session-1",
    streamId: "agent-output",
    sequence: 18,
    cursor: "cursor-18",
    eventId: "event-18",
    requestId: "request-2",
    kind: "agent_message_content",
    payload: { text: "canonical chunk" },
  };
  transport.receive({ jsonrpc: "2.0", method: "session/update", params: { sessionId: "session-1", update: { event: { ...canonical } } } });
  transport.receive({ jsonrpc: "2.0", method: "session/update", params: { sessionId: "session-1", update: { sessionUpdate: "agent_message_chunk", streamId: "legacy-output", text: "first" } } });
  transport.receive({ jsonrpc: "2.0", method: "session/update", params: { sessionId: "session-1", update: { sessionUpdate: "agent_message_chunk", streamId: "legacy-output", text: "second" } } });

  assert.deepEqual(events, [
    canonical,
    { type: "event", sessionId: "session-1", streamId: "legacy-output", sequence: 1, cursor: "1", eventId: "legacy-output:1", kind: "agent_message_chunk", payload: { sessionUpdate: "agent_message_chunk", streamId: "legacy-output", text: "first" } },
    { type: "event", sessionId: "session-1", streamId: "legacy-output", sequence: 2, cursor: "2", eventId: "legacy-output:2", kind: "agent_message_chunk", payload: { sessionUpdate: "agent_message_chunk", streamId: "legacy-output", text: "second" } },
  ]);
});

test("client answers permission, legacy input, and ACP form elicitation requests with protocol outcomes", async (t) => {
  const permissionCalls: unknown[] = [];
  const inputCalls: unknown[] = [];
  const inputResults: Array<string | undefined> = ["typed value", "form answer", undefined];
  const interaction: ClientInteraction = {
    async requestPermission(request) { permissionCalls.push(request); return "allow-once"; },
    async requestInput(request) { inputCalls.push(request); return inputResults.shift(); },
  };
  const transport = new MockAcpTransport(standardAgent);
  const client = createClient(() => transport, interaction);
  t.after(async () => client.dispose());
  await client.connect();

  transport.receive({
    jsonrpc: "2.0",
    id: 70,
    method: "session/request_permission",
    params: {
      sessionId: "session-1",
      toolCall: { title: "Write file", path: "/workspace/a.ts" },
      options: [
        { optionId: "allow-once", name: "Allow once", kind: "allow_once" },
        { optionId: "reject", name: "Reject", kind: "reject_once" },
        { invalid: true },
      ],
    },
  });
  assert.deepEqual(await waitForSent(transport, (message) => "id" in message && message.id === 70), {
    jsonrpc: "2.0",
    id: 70,
    result: { outcome: { outcome: "selected", optionId: "allow-once" } },
  });
  assert.deepEqual(permissionCalls, [{
    sessionId: "session-1",
    toolCall: { title: "Write file", path: "/workspace/a.ts" },
    options: [
      { optionId: "allow-once", name: "Allow once", kind: "allow_once" },
      { optionId: "reject", name: "Reject", kind: "reject_once" },
    ],
  }]);

  transport.receive({ jsonrpc: "2.0", id: 71, method: "session/requestInput", params: { sessionId: "session-1", prompt: "Branch name", placeholder: "feature/name", password: false } });
  assert.deepEqual(await waitForSent(transport, (message) => "id" in message && message.id === 71), {
    jsonrpc: "2.0",
    id: 71,
    result: { outcome: "submitted", value: "typed value" },
  });
  assert.deepEqual(inputCalls, [{ sessionId: "session-1", prompt: "Branch name", placeholder: "feature/name", password: false }]);

  const requestedSchema = {
    type: "object",
    properties: { answer: { type: "string", title: "Release channel" } },
    required: ["answer"],
  };
  transport.receive({ jsonrpc: "2.0", id: 72, method: "elicitation/create", params: { mode: "form", sessionId: "session-1", requestedSchema, message: "Choose a release channel" } });
  assert.deepEqual(await waitForSent(transport, (message) => "id" in message && message.id === 72), {
    jsonrpc: "2.0",
    id: 72,
    result: { action: "accept", content: { answer: "form answer" } },
  });

  transport.receive({ jsonrpc: "2.0", id: 73, method: "elicitation/create", params: { mode: "form", sessionId: "session-1", requestedSchema, message: "Choose a release channel" } });
  assert.deepEqual(await waitForSent(transport, (message) => "id" in message && message.id === 73), {
    jsonrpc: "2.0",
    id: 73,
    result: { action: "cancel" },
  });
});

test("unexpected disconnect reconnects, reinitializes, and reloads the active session before resume", async (t) => {
  const transports: MockAcpTransport[] = [];
  const factory = (): MockAcpTransport => {
    const transport = new MockAcpTransport(standardAgent);
    transports.push(transport);
    return transport;
  };
  const client = createClient(factory, undefined, { enabled: true, limit: 1 });
  t.after(async () => client.dispose());
  await client.connect();
  await client.newSession("/workspace/resume");
  const resumed = once(client, "resumed");

  const first = transports[0];
  assert.ok(first);
  const realSetTimeout = globalThis.setTimeout;
  globalThis.setTimeout = ((callback: (...args: never[]) => void) => {
    callback();
    return 0;
  }) as unknown as typeof setTimeout;
  try { first.disconnect(false); }
  finally { globalThis.setTimeout = realSetTimeout; }
  const [resumedSession] = await resumed;

  assert.deepEqual(resumedSession, { id: "session-new", cwd: "/workspace/resume" });
  assert.equal(transports.length, 2);
  const second = transports[1];
  assert.ok(second);
  const methods = second.sent.filter(isRequest).map((message) => message.method);
  assert.deepEqual(methods, ["initialize", "session/load"]);
  assert.deepEqual(second.sent.filter(isRequest).at(-1), {
    jsonrpc: "2.0",
    id: 4,
    method: "session/load",
    params: { sessionId: "session-new", cwd: "/workspace/resume", mcpServers: [] },
  });
  assert.deepEqual(client.session, { id: "session-new", cwd: "/workspace/resume" });
});
