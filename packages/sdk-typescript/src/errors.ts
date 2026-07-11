import type { ErrorEnvelope, JsonValue } from "./types.js";

export class EnvelopeValidationError extends TypeError {
  constructor(message: string) {
    super(message);
    this.name = "EnvelopeValidationError";
  }
}

export class ProtocolError extends Error {
  readonly code: string;
  readonly retryable: boolean;
  readonly details: JsonValue;
  readonly requestId: string | undefined;

  constructor(envelope: ErrorEnvelope) {
    super(envelope.error.message);
    this.name = "ProtocolError";
    this.code = envelope.error.code;
    this.retryable = envelope.error.retryable;
    this.details = envelope.error.details;
    this.requestId = envelope.id;
  }
}

export class TransportClosedError extends Error {
  constructor(message = "The session transport closed before a response arrived") {
    super(message);
    this.name = "TransportClosedError";
  }
}
