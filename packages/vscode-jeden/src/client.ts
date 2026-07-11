import { EventEmitter } from "node:events";
import type { AcpTransport } from "./transport.js";
import { ACP_PROTOCOL_VERSION, asJsonValue, isFailure, isNotification, isObject, isRequest, type AgentCapabilities, type InitializeResult, type InputRequest, type JsonObject, type JsonRpcMessage, type JsonValue, type PermissionRequest, type SessionEventEnvelope } from "./protocol.js";
import type { RedactingLogger } from "./logging.js";

export interface ClientInteraction {
  requestPermission(request: PermissionRequest): Promise<string | undefined>;
  requestInput(request: InputRequest): Promise<string | undefined>;
  cancelPending?(): void;
}
export interface ClientOptions {
  readonly transport: () => AcpTransport;
  readonly interaction: ClientInteraction;
  readonly logger: RedactingLogger;
  readonly autoReconnect: () => boolean;
  readonly reconnectLimit: () => number;
}
interface PendingRequest { readonly method: string; readonly resolve: (value: JsonValue) => void; readonly reject: (error: Error) => void; }
export interface ActiveSession { readonly id: string; readonly cwd: string; }

export class AcpError extends Error { constructor(readonly code: number, message: string, readonly data?: JsonValue) { super(message); this.name = "AcpError"; } }

export class JedenAcpClient extends EventEmitter {
  private transport: AcpTransport | undefined;
  private pending = new Map<number, PendingRequest>();
  private nextId = 1;
  private generation = 0;
  private reconnectAttempt = 0;
  private disposed = false;
  private connecting: Promise<InitializeResult> | undefined;
  private initialized: InitializeResult | undefined;

  private sequenceByStream = new Map<string, number>();
  capabilities: AgentCapabilities | undefined;
  session: ActiveSession | undefined;
  turnActive = false;

  constructor(private readonly options: ClientOptions) { super(); }

  connect(): Promise<InitializeResult> {
    if (this.initialized && this.transport) return Promise.resolve(this.initialized);
    if (this.connecting) return this.connecting;
    const operation = this.establish().finally(() => { if (this.connecting === operation) this.connecting = undefined; });
    this.connecting = operation;
    return operation;
  }

  private async establish(): Promise<InitializeResult> {
    this.disposed = false;
    const generation = ++this.generation;
    const transport = this.options.transport();
    this.transport = transport;
    transport.events.on("message", (message: JsonRpcMessage) => {
      if (generation !== this.generation) return;
      void this.receive(message).catch((error: unknown) => {
        this.options.logger.error("acp.receive.error", error);
        this.emit("error", error instanceof Error ? error : new Error("ACP receive failed"));
      });
    });
    transport.events.on("error", (error: Error) => { if (generation === this.generation) this.emit("error", error); });
    transport.events.on("close", (info: { intentional: boolean }) => { if (generation === this.generation) this.closed(info.intentional); });
    await transport.start();
    const result = await this.request("initialize", {
      protocolVersion: ACP_PROTOCOL_VERSION,
      clientCapabilities: { elicitation: { form: {} } },
      clientInfo: { name: "vscode-jeden", version: "0.1.0" },
    });
    if (!isObject(result) || typeof result.protocolVersion !== "number" || !isObject(result.agentCapabilities)) throw new Error("ACP initialize returned invalid capabilities");
    const raw = result.agentCapabilities;
    const prompt = isObject(raw.promptCapabilities) ? raw.promptCapabilities : {};
    this.capabilities = {
      loadSession: raw.loadSession === true,
      promptCapabilities: { image: prompt.image === true, audio: prompt.audio === true, embeddedContext: prompt.embeddedContext === true },
      sessionCapabilities: isObject(raw.sessionCapabilities) ? asJsonValue(raw.sessionCapabilities) as JsonObject : {},
    };
    const initialized: InitializeResult = { protocolVersion: result.protocolVersion, agentCapabilities: this.capabilities, ...(isObject(result.agentInfo) && typeof result.agentInfo.name === "string" && typeof result.agentInfo.version === "string" ? { agentInfo: { name: result.agentInfo.name, version: result.agentInfo.version } } : {}) };
    this.initialized = initialized;
    this.emit("connected", initialized);
    return initialized;
  }

  async newSession(cwd: string): Promise<ActiveSession> {
    const result = await this.request("session/new", { cwd, mcpServers: [] });
    if (!isObject(result) || typeof result.sessionId !== "string") throw new Error("ACP session/new returned no sessionId");
    this.session = { id: result.sessionId, cwd };
    this.emit("session", this.session);
    return this.session;
  }

  async loadSession(sessionId: string, cwd: string): Promise<ActiveSession> {
    this.requireCapability("sessionLoad");
    await this.request("session/load", { sessionId, cwd, mcpServers: [] });
    this.session = { id: sessionId, cwd };
    this.emit("session", this.session);
    return this.session;
  }

  async prompt(text: string): Promise<JsonValue> {
    this.requireCapability("prompt");
    if (!this.session) throw new Error("No active Jeden session");
    if (text.trim().length === 0) throw new Error("Prompt cannot be empty");
    this.turnActive = true;
    this.emit("turn", true);
    try { return await this.request("session/prompt", { sessionId: this.session.id, prompt: [{ type: "text", text }] }); }
    finally { this.turnActive = false; this.emit("turn", false); }
  }

  async cancel(): Promise<void> {
    this.requireCapability("cancel");
    if (!this.session) return;
    this.options.interaction.cancelPending?.();
    await this.notify("session/cancel", { sessionId: this.session.id });
  }

  hasCapability(capability: "sessionNew" | "sessionLoad" | "prompt" | "cancel" | "permission" | "input"): boolean {
    if (!this.capabilities) return false;
    if (capability === "sessionLoad") return this.capabilities.loadSession;
    if (capability === "permission" || capability === "input") return true;
    return true;
  }

  requireCapability(capability: Parameters<JedenAcpClient["hasCapability"]>[0]): void { if (!this.hasCapability(capability)) throw new AcpError(-32601, `ACP capability is unavailable: ${capability}`); }

  private request(method: string, params: JsonValue): Promise<JsonValue> {
    const transport = this.transport;
    if (!transport) return Promise.reject(new Error("ACP client is disconnected"));
    const id = this.nextId++;
    return new Promise<JsonValue>((resolve, reject) => {
      this.pending.set(id, { method, resolve, reject });
      void transport.send({ jsonrpc: "2.0", id, method, params }).catch((error: unknown) => { this.pending.delete(id); reject(error instanceof Error ? error : new Error("ACP send failed")); });
    });
  }

  private async notify(method: string, params: JsonValue): Promise<void> {
    if (!this.transport) throw new Error("ACP client is disconnected");
    await this.transport.send({ jsonrpc: "2.0", method, params });
  }

  private async receive(message: JsonRpcMessage): Promise<void> {
    if (isRequest(message)) { await this.receiveRequest(message); return; }
    if (isNotification(message)) {
      if (message.method === "session/update" && isObject(message.params) && typeof message.params.sessionId === "string" && isObject(message.params.update)) this.emitSessionUpdate(message.params.sessionId, message.params.update);
      return;
    }
    if (message.id === null) return;
    const pending = this.pending.get(message.id);
    if (!pending) return;
    this.pending.delete(message.id);
    if (isFailure(message)) pending.reject(new AcpError(message.error.code, message.error.message, message.error.data)); else pending.resolve(message.result);
  }

  private async receiveRequest(message: Extract<JsonRpcMessage, { readonly method: string; readonly id: number }>): Promise<void> {
    try {
      if ((message.method === "session/request_permission" || message.method === "session/requestPermission") && isObject(message.params)) {
        const params = message.params;
        const options: Array<{ optionId: string; name: string; kind: string }> = [];
        if (Array.isArray(params.options)) {
          for (const candidate of params.options) {
            if (!isObject(candidate) || typeof candidate.optionId !== "string") continue;
            options.push({ optionId: candidate.optionId, name: typeof candidate.name === "string" ? candidate.name : candidate.optionId, kind: typeof candidate.kind === "string" ? candidate.kind : "" });
          }
        }
        const selected = await this.options.interaction.requestPermission({ sessionId: typeof params.sessionId === "string" ? params.sessionId : "", toolCall: isObject(params.toolCall) ? asJsonValue(params.toolCall) as JsonObject : {}, options });
        await this.respond(message.id, selected ? { outcome: { outcome: "selected", optionId: selected } } : { outcome: { outcome: "cancelled" } }); return;
      }
      if (message.method === "elicitation/create" && isObject(message.params)) {
        const params = message.params;
        if (params.mode !== "form") {
          await this.respondError(message.id, -32602, "Only ACP form elicitation is supported");
          return;
        }
        const schema = isObject(params.requestedSchema) ? params.requestedSchema : {};
        const properties = isObject(schema.properties) ? schema.properties : {};
        const field = Object.keys(properties)[0] ?? "answer";
        const value = await this.options.interaction.requestInput({
          sessionId: typeof params.sessionId === "string" ? params.sessionId : "",
          prompt: typeof params.message === "string" ? params.message : "Jeden requires input",
        });
        await this.respond(message.id, value === undefined ? { action: "cancel" } : { action: "accept", content: { [field]: value } });
        return;
      }
      if ((message.method === "session/request_input" || message.method === "session/requestInput") && isObject(message.params)) {
        const params = message.params; const value = await this.options.interaction.requestInput({ sessionId: typeof params.sessionId === "string" ? params.sessionId : "", prompt: typeof params.prompt === "string" ? params.prompt : "Jeden requires input", ...(typeof params.placeholder === "string" ? { placeholder: params.placeholder } : {}), ...(typeof params.password === "boolean" ? { password: params.password } : {}) });
        await this.respond(message.id, value === undefined ? { outcome: "cancelled" } : { outcome: "submitted", value }); return;
      }
      await this.respondError(message.id, -32601, "Method not supported by VS Code Jeden client");
    } catch (error) { this.options.logger.error("acp.client_request.error", error); await this.respondError(message.id, -32603, "Client interaction failed"); }
  }

  private async respond(id: number, result: JsonValue): Promise<void> { if (this.transport) await this.transport.send({ jsonrpc: "2.0", id, result }); }
  private async respondError(id: number, code: number, message: string): Promise<void> { if (this.transport) await this.transport.send({ jsonrpc: "2.0", id, error: { code, message } }); }

  private emitSessionUpdate(sessionId: string, update: Record<string, unknown>): void {
    const canonical = isObject(update.event) && update.event.type === "event" ? update.event : undefined;
    let event: SessionEventEnvelope;
    if (canonical && typeof canonical.sessionId === "string" && typeof canonical.streamId === "string" && typeof canonical.sequence === "number" && typeof canonical.cursor === "string" && typeof canonical.eventId === "string" && typeof canonical.kind === "string") {
      event = { type: "event", sessionId: canonical.sessionId, streamId: canonical.streamId, sequence: canonical.sequence, cursor: canonical.cursor, eventId: canonical.eventId, ...(typeof canonical.requestId === "string" ? { requestId: canonical.requestId } : {}), kind: canonical.kind, payload: asJsonValue(canonical.payload ?? {}) };
    } else {
      const kind = typeof update.sessionUpdate === "string" ? update.sessionUpdate : typeof update.kind === "string" ? update.kind : "session.update";
      const streamId = typeof update.streamId === "string" ? update.streamId : sessionId;
      const sequence = (this.sequenceByStream.get(streamId) ?? 0) + 1; this.sequenceByStream.set(streamId, sequence);
      event = { type: "event", sessionId, streamId, sequence, cursor: typeof update.cursor === "string" ? update.cursor : String(sequence), eventId: typeof update.eventId === "string" ? update.eventId : `${streamId}:${sequence}`, kind, payload: asJsonValue(update) };
    }
    this.emit("event", event);
  }

  private closed(intentional: boolean): void {
    this.transport = undefined; this.capabilities = undefined; this.initialized = undefined;
    for (const pending of this.pending.values()) pending.reject(new Error("ACP connection closed"));
    this.pending.clear(); this.emit("disconnected", intentional);
    if (!intentional && !this.disposed && this.options.autoReconnect() && this.reconnectAttempt < this.options.reconnectLimit()) {
      const attempt = ++this.reconnectAttempt; const session = this.session;
      setTimeout(() => {
        void this.connect().then(async () => {
          if (session) {
            if (!this.hasCapability("sessionLoad")) {
              this.session = undefined;
              this.reconnectAttempt = 0;
              this.emit("resumed", undefined);
              return;
            }
            await this.loadSession(session.id, session.cwd);
          }
          this.reconnectAttempt = 0;
          this.emit("resumed", session);
        }).catch((error: unknown) => {
          this.options.logger.error("acp.reconnect.error", error);
          this.closed(false);
        });
      }, Math.min(250 * 2 ** (attempt - 1), 4_000));
    }
  }

  async dispose(): Promise<void> {
    this.disposed = true; this.generation++; this.capabilities = undefined; this.initialized = undefined; this.session = undefined;
    for (const pending of this.pending.values()) pending.reject(new Error("ACP client disposed"));
    this.pending.clear(); const transport = this.transport; this.transport = undefined; if (transport) await transport.close();
  }
}
