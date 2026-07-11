import { EnvelopeValidationError } from "./errors.js";
import {
  PROTOCOL_VERSION,
  type Envelope,
  type ErrorEnvelope,
  type EventEnvelope,
  type JsonValue,
  type RequestEnvelope,
  type ResponseEnvelope,
} from "./types.js";

const REQUEST_KEYS = ["type", "id", "method", "params", "meta"] as const;
const META_KEYS = ["protocolVersion", "idempotencyKey", "deadline", "traceId"] as const;
const RESPONSE_KEYS = ["type", "id", "result"] as const;
const EVENT_KEYS = [
  "type", "sessionId", "streamId", "sequence", "cursor", "eventId",
  "requestId", "kind", "payload",
] as const;
const ERROR_ENVELOPE_KEYS = ["type", "id", "error"] as const;
const ERROR_KEYS = ["code", "message", "retryable", "details"] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const prototype: unknown = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function hasOnlyKeys(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  return Object.keys(value).every((key) => allowed.includes(key));
}

function hasOwn(value: Record<string, unknown>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function hasExactRequiredKeys(
  value: Record<string, unknown>,
  allowed: readonly string[],
  required: readonly string[],
): boolean {
  return hasOnlyKeys(value, allowed) && required.every((key) => hasOwn(value, key));
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0;
}

function isOptionalNonEmptyString(value: Record<string, unknown>, key: string): boolean {
  return !hasOwn(value, key) || isNonEmptyString(value[key]);
}

function isReplayParams(value: unknown): boolean {
  if (!isRecord(value) || !hasExactRequiredKeys(value, ["sessionId", "cursor", "limit"], ["sessionId"])) return false;
  return isNonEmptyString(value.sessionId)
    && (!hasOwn(value, "cursor") || isNonEmptyString(value.cursor))
    && (!hasOwn(value, "limit")
      || (typeof value.limit === "number" && Number.isInteger(value.limit) && value.limit >= 1));
}

export function isJsonValue(value: unknown): value is JsonValue {
  return isJsonValueInternal(value, new WeakSet<object>());
}

function isJsonValueInternal(value: unknown, ancestors: WeakSet<object>): value is JsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (typeof value !== "object") return false;
  if (ancestors.has(value)) return false;
  ancestors.add(value);
  const valid = Array.isArray(value)
    ? value.every((entry) => isJsonValueInternal(entry, ancestors))
    : isRecord(value) && Object.values(value).every((entry) => isJsonValueInternal(entry, ancestors));
  ancestors.delete(value);
  return valid;
}

export function isRequestEnvelope(value: unknown): value is RequestEnvelope {
  if (!isRecord(value) || !hasExactRequiredKeys(value, REQUEST_KEYS, REQUEST_KEYS)) return false;
  if (value.type !== "request" || !isNonEmptyString(value.id) || !isNonEmptyString(value.method)) return false;
  if (!isJsonValue(value.params) || !isRecord(value.meta)) return false;
  if (value.method === "session.replay" && !isReplayParams(value.params)) return false;
  const meta = value.meta;
  return hasExactRequiredKeys(meta, META_KEYS, ["protocolVersion", "idempotencyKey"])
    && meta.protocolVersion === PROTOCOL_VERSION
    && isNonEmptyString(meta.idempotencyKey)
    && isOptionalNonEmptyString(meta, "deadline")
    && isOptionalNonEmptyString(meta, "traceId");
}

export function isResponseEnvelope(value: unknown): value is ResponseEnvelope {
  return isRecord(value)
    && hasExactRequiredKeys(value, RESPONSE_KEYS, RESPONSE_KEYS)
    && value.type === "response"
    && isNonEmptyString(value.id)
    && isJsonValue(value.result);
}

export function isEventEnvelope(value: unknown): value is EventEnvelope {
  if (!isRecord(value) || !hasExactRequiredKeys(
    value,
    EVENT_KEYS,
    ["type", "sessionId", "streamId", "sequence", "cursor", "eventId", "kind", "payload"],
  )) return false;
  return value.type === "event"
    && isNonEmptyString(value.sessionId)
    && isNonEmptyString(value.streamId)
    && Number.isInteger(value.sequence)
    && typeof value.sequence === "number"
    && value.sequence >= 0
    && isNonEmptyString(value.cursor)
    && isNonEmptyString(value.eventId)
    && (!hasOwn(value, "requestId") || isNonEmptyString(value.requestId))
    && isNonEmptyString(value.kind)
    && isJsonValue(value.payload);
}

export function isErrorEnvelope(value: unknown): value is ErrorEnvelope {
  if (!isRecord(value) || !hasExactRequiredKeys(value, ERROR_ENVELOPE_KEYS, ["type", "error"])) return false;
  if (value.type !== "error" || (hasOwn(value, "id") && !isNonEmptyString(value.id)) || !isRecord(value.error)) return false;
  const error = value.error;
  return hasExactRequiredKeys(error, ERROR_KEYS, ERROR_KEYS)
    && typeof error.code === "string"
    && typeof error.message === "string"
    && typeof error.retryable === "boolean"
    && isJsonValue(error.details);
}

export function isEnvelope(value: unknown): value is Envelope {
  if (!isRecord(value)) return false;
  switch (value.type) {
    case "request": return isRequestEnvelope(value);
    case "response": return isResponseEnvelope(value);
    case "event": return isEventEnvelope(value);
    case "error": return isErrorEnvelope(value);
    default: return false;
  }
}

export function parseEnvelope(value: unknown): Envelope {
  if (!isEnvelope(value)) {
    throw new EnvelopeValidationError("Malformed jeden.session.v1 envelope");
  }
  return value;
}
