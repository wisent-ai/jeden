import { EventEmitter } from "node:events";
import type { JsonObject, JsonValue, SessionEventEnvelope } from "./protocol.js";

export interface Artifact { readonly id: string; readonly name: string; readonly uri: string; readonly mediaType?: string; }
export interface Job { readonly id: string; readonly label: string; readonly state: string; readonly detail?: string; }
export interface DiagnosticRecord { readonly id: string; readonly uri: string; readonly message: string; readonly severity: string; readonly line: number; readonly character: number; }
export interface PendingAction { readonly id: string; readonly title: string; readonly kind: "approval" | "diff" | "input"; readonly uri?: string; readonly before?: string; readonly after?: string; readonly detail?: string; }
export interface TranscriptItem { readonly role: "agent" | "user" | "status" | "tool" | "plan"; readonly text: string; }

function object(value: JsonValue): JsonObject | undefined { return typeof value === "object" && value !== null && !Array.isArray(value) ? value : undefined; }
function text(value: JsonValue | undefined): string | undefined { return typeof value === "string" ? value : undefined; }
function numberValue(value: JsonValue | undefined): number | undefined { return typeof value === "number" ? value : undefined; }
function contentText(payload: JsonObject): string | undefined {
  const content = payload.content === undefined ? undefined : object(payload.content);
  return text(payload.text) ?? text(payload.content) ?? (content ? text(content.text) : undefined);
}


export class ExtensionModel extends EventEmitter {
  readonly transcript: TranscriptItem[] = [];
  readonly artifacts = new Map<string, Artifact>();
  readonly jobs = new Map<string, Job>();
  readonly diagnostics = new Map<string, DiagnosticRecord>();
  readonly pending = new Map<string, PendingAction>();
  modelStatus = "No model";
  accountStatus = "No account";
  serviceStatus = "Disconnected";

  apply(event: SessionEventEnvelope): void {
    const payload = object(event.payload) ?? {};
    const kind = event.kind;
    switch (kind) {
      case "agent_message_content":
      case "agent_message_chunk": {
        const value = contentText(payload);
        if (value !== undefined) this.transcript.push({ role: "agent", text: value });
        break;
      }
      case "agent_thought":
      case "agent_thought_chunk": {
        const value = contentText(payload);
        if (value !== undefined) this.transcript.push({ role: "status", text: value });
        break;
      }
      case "tool_call":
      case "tool_call_update":
        this.transcript.push({ role: "tool", text: text(payload.title) ?? text(payload.status) ?? kind });
        break;
      case "plan_update":
      case "plan": {
        const entries = Array.isArray(payload.entries)
          ? payload.entries.flatMap((entry) => { const value = object(entry); const content = value ? text(value.content) : undefined; return content === undefined ? [] : [content]; })
          : [];
        this.transcript.push({ role: "plan", text: text(payload.text) ?? text(payload.title) ?? (entries.length > 0 ? entries.join("\n") : "Plan updated") });
        break;
      }
      case "artifact_created": {
        const id = text(payload.id) ?? event.eventId;
        const uri = text(payload.uri);
        const mediaType = text(payload.mediaType);
        if (uri) this.artifacts.set(id, { id, name: text(payload.name) ?? id, uri, ...(mediaType === undefined ? {} : { mediaType }) });
        break;
      }
      case "worker_job_update": {
        const id = text(payload.id) ?? text(payload.jobId) ?? event.eventId;
        const detail = text(payload.detail);
        this.jobs.set(id, { id, label: text(payload.label) ?? id, state: text(payload.state) ?? text(payload.status) ?? "updated", ...(detail === undefined ? {} : { detail }) });
        break;
      }
      case "diagnostic_update": {
        const id = text(payload.id) ?? event.eventId;
        const uri = text(payload.uri);
        const message = text(payload.message);
        if (uri && message) this.diagnostics.set(id, { id, uri, message, severity: text(payload.severity) ?? "warning", line: numberValue(payload.line) ?? 0, character: numberValue(payload.character) ?? 0 });
        break;
      }
      case "pending_diff": {
        const id = text(payload.id) ?? event.eventId;
        const uri = text(payload.uri);
        const before = text(payload.before);
        const after = text(payload.after);
        const detail = text(payload.detail);
        if (uri && before !== undefined && after !== undefined) this.pending.set(id, { id, kind: "diff", title: text(payload.title) ?? "Pending diff", uri, before, after, ...(detail === undefined ? {} : { detail }) });
        break;
      }
      case "model_status":
        this.modelStatus = text(payload.label) ?? text(payload.status) ?? this.modelStatus;
        break;
      case "account_status":
        this.accountStatus = text(payload.label) ?? text(payload.status) ?? this.accountStatus;
        break;
      case "service_status":
        this.serviceStatus = text(payload.message) ?? text(payload.status) ?? this.serviceStatus;
        break;
    }
    this.emit("change", event);
  }
  clearSession(): void { this.transcript.length = 0; this.artifacts.clear(); this.jobs.clear(); this.diagnostics.clear(); this.pending.clear(); this.emit("change"); }
}
