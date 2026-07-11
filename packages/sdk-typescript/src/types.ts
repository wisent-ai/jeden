export const PROTOCOL_VERSION = "jeden.session.v1" as const;

export type JsonPrimitive = null | boolean | number | string;
export type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };

export interface RequestMeta {
  protocolVersion: typeof PROTOCOL_VERSION;
  idempotencyKey: string;
  deadline?: string;
  traceId?: string;
}

export interface RequestEnvelope<Params extends JsonValue = JsonValue> {
  type: "request";
  id: string;
  method: string;
  params: Params;
  meta: RequestMeta;
}

export interface ResponseEnvelope<Result extends JsonValue = JsonValue> {
  type: "response";
  id: string;
  result: Result;
}

export interface EventEnvelope<Payload extends JsonValue = JsonValue> {
  type: "event";
  sessionId: string;
  streamId: string;
  sequence: number;
  cursor: string;
  eventId: string;
  requestId?: string;
  kind: string;
  payload: Payload;
}

export interface ErrorBody {
  code: string;
  message: string;
  retryable: boolean;
  details: JsonValue;
}

export interface ErrorEnvelope {
  type: "error";
  id?: string;
  error: ErrorBody;
}

export type Envelope =
  | RequestEnvelope
  | ResponseEnvelope
  | EventEnvelope
  | ErrorEnvelope;

export interface ReplayParams {
  sessionId: string;
  cursor?: string;
  limit?: number;
}
