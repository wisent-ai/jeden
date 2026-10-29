export const ACP_PROTOCOL_VERSION = 1 as const;
export const SESSION_PROTOCOL_VERSION = "jeden.session.v1" as const;

export type JsonPrimitive = null | boolean | number | string;
export type JsonValue = JsonPrimitive | JsonValue[] | { readonly [key: string]: JsonValue };
export type JsonObject = { readonly [key: string]: JsonValue };

// JSON-RPC 2.0 lets a request or notification omit params; an absent one stays absent, never read as `{}`.
export interface JsonRpcRequest { readonly jsonrpc: "2.0"; readonly id: number; readonly method: string; readonly params?: JsonValue; }
export interface JsonRpcNotification { readonly jsonrpc: "2.0"; readonly method: string; readonly params?: JsonValue; }
export interface JsonRpcSuccess { readonly jsonrpc: "2.0"; readonly id: number; readonly result: JsonValue; }
export interface JsonRpcFailure { readonly jsonrpc: "2.0"; readonly id: number | null; readonly error: { readonly code: number; readonly message: string; readonly data?: JsonValue }; }
export type JsonRpcMessage = JsonRpcRequest | JsonRpcNotification | JsonRpcSuccess | JsonRpcFailure;

/** Canonical protocol/schema/v1/envelope.schema.json event shape. */
export interface SessionEventEnvelope {
  readonly type: "event";
  readonly sessionId: string;
  readonly streamId: string;
  readonly sequence: number;
  readonly cursor: string;
  readonly eventId: string;
  readonly requestId?: string;
  readonly kind: string;
  readonly payload: JsonValue;
}

export interface AgentCapabilities {
  readonly loadSession: boolean;
  readonly promptCapabilities: { readonly image: boolean; readonly audio: boolean; readonly embeddedContext: boolean };
  readonly sessionCapabilities: Readonly<Record<string, JsonValue>>;
}

export interface InitializeResult { readonly protocolVersion: number; readonly agentCapabilities: AgentCapabilities; readonly agentInfo?: { readonly name: string; readonly version: string }; }
export interface SessionUpdate { readonly sessionId: string; readonly update: JsonObject; }
export interface PermissionOption { readonly optionId: string; readonly name: string; readonly kind: string; }
export interface PermissionRequest { readonly sessionId: string; readonly toolCall: JsonObject; readonly options: readonly PermissionOption[]; }
export interface InputRequest { readonly sessionId: string; readonly prompt: string; readonly placeholder?: string; readonly password?: boolean; }

export function isObject(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value); }
/** Whether a decoded JSON value is a string, a number or a boolean: the JSON type, read from the value. */
export function isText(value: unknown): value is string { return typeof value === "string"; }
export function isCount(value: unknown): value is number { return typeof value === "number"; }
export function isFlag(value: unknown): value is boolean { return typeof value === "boolean"; }
/** The string `field` of `params`; a message that lacks it is refused naming `where` and the field, never filled in. */
export function required(params: Record<string, unknown>, field: string, where: string): string {
  const value = params[field];
  if (!isText(value)) throw new Error(`${where} carries no ${field}`);
  return value;
}
export function asJsonValue(value: unknown): JsonValue { if (value === null || isText(value) || isFlag(value) || (isCount(value) && Number.isFinite(value))) return value; if (Array.isArray(value)) return value.map(asJsonValue); if (isObject(value)) { const out: Record<string, JsonValue> = {}; for (const [key, child] of Object.entries(value)) out[key] = asJsonValue(child); return out; } throw new Error("ACP message contains a non-JSON value"); }
export function parseMessage(value: unknown): JsonRpcMessage {
  if (!isObject(value) || value.jsonrpc !== "2.0") throw new Error("Invalid ACP JSON-RPC envelope");
  if (typeof value.method === "string") {
    const params = value.params === undefined ? {} : { params: asJsonValue(value.params) };
    if (typeof value.id === "number" && Number.isInteger(value.id)) { const id = value.id; const method = value.method; return { jsonrpc: "2.0", id, method, ...params }; }
    if (value.id === undefined) { const method = value.method; return { jsonrpc: "2.0", method, ...params }; }
  }
  if ((typeof value.id === "number" && Number.isInteger(value.id)) || value.id === null) {
    if ("error" in value && isObject(value.error) && isCount(value.error.code) && isText(value.error.message)) {
      const code = value.error.code;
      const message = value.error.message;
      const data = value.error.data;
      const id = typeof value.id === "number" ? value.id : null;
      return { jsonrpc: "2.0", id, error: { code, message, ...(data === undefined ? {} : { data: asJsonValue(data) }) } };
    }
    if (typeof value.id === "number" && "result" in value) return { jsonrpc: "2.0", id: value.id, result: asJsonValue(value.result) };
  }
  throw new Error("Invalid ACP JSON-RPC message");
}

export function isRequest(message: JsonRpcMessage): message is JsonRpcRequest { return "method" in message && "id" in message; }
export function isNotification(message: JsonRpcMessage): message is JsonRpcNotification { return "method" in message && !("id" in message); }
export function isFailure(message: JsonRpcMessage): message is JsonRpcFailure { return "error" in message; }
