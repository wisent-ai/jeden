import { EnvelopeValidationError, ProtocolError, TransportClosedError } from "./errors.js";
import type { AsyncEnvelopeTransport } from "./transport.js";
import {
  PROTOCOL_VERSION,
  type EventEnvelope,
  type JsonValue,
  type ReplayParams,
  type RequestEnvelope,
} from "./types.js";
import { isJsonValue, isRequestEnvelope, parseEnvelope } from "./validators.js";

export interface RequestOptions {
  /** Required for every call so retries cannot duplicate mutation side effects. */
  idempotencyKey: string;
  deadline?: string;
  traceId?: string;
  id?: string;
}

interface PendingRequest {
  resolve(value: JsonValue): void;
  reject(reason: unknown): void;
}

interface WaitingEvent {
  resolve(result: IteratorResult<EventEnvelope>): void;
  reject(reason: unknown): void;
}

class EventQueue implements AsyncIterableIterator<EventEnvelope> {
  readonly #values: EventEnvelope[] = [];
  readonly #waiters: WaitingEvent[] = [];
  #closed = false;
  #failure: unknown;
  #failed = false;

  [Symbol.asyncIterator](): AsyncIterableIterator<EventEnvelope> {
    return this;
  }

  next(): Promise<IteratorResult<EventEnvelope>> {
    const value = this.#values.shift();
    if (value !== undefined) return Promise.resolve({ done: false, value });
    if (this.#failed) return Promise.reject(this.#failure);
    if (this.#closed) return Promise.resolve({ done: true, value: undefined });
    const { promise, resolve, reject } = Promise.withResolvers<IteratorResult<EventEnvelope>>();
    this.#waiters.push({ resolve, reject });
    return promise;
  }

  push(value: EventEnvelope): void {
    if (this.#closed || this.#failed) return;
    const waiter = this.#waiters.shift();
    if (waiter !== undefined) waiter.resolve({ done: false, value });
    else this.#values.push(value);
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    for (const waiter of this.#waiters.splice(0)) waiter.resolve({ done: true, value: undefined });
  }

  fail(reason: unknown): void {
    if (this.#closed || this.#failed) return;
    this.#failed = true;
    this.#failure = reason;
    for (const waiter of this.#waiters.splice(0)) waiter.reject(reason);
  }
}

export class SessionClient {
  readonly #pending = new Map<string, PendingRequest>();
  readonly #events = new EventQueue();
  #nextId = 1;
  #started = false;
  #closed = false;

  constructor(private readonly transport: AsyncEnvelopeTransport) {}

  /** A lossless, arrival-ordered event iterator preserving sequence and cursor values. */
  events(): AsyncIterableIterator<EventEnvelope> {
    this.#start();
    return this.#events;
  }

  async request<Result extends JsonValue = JsonValue, Params extends JsonValue = JsonValue>(
    method: string,
    params: Params,
    options: RequestOptions,
  ): Promise<Result> {
    if (this.#closed) throw new TransportClosedError("The session client is closed");
    if (typeof method !== "string" || method.length === 0) throw new TypeError("method must be a non-empty string");
    if (!isJsonValue(params)) throw new TypeError("params must be a JSON value");
    if (typeof options.idempotencyKey !== "string" || options.idempotencyKey.length === 0) {
      throw new TypeError("idempotencyKey must be a non-empty string");
    }
    if (options.deadline !== undefined && options.deadline.length === 0) throw new TypeError("deadline must be non-empty");
    if (options.traceId !== undefined && options.traceId.length === 0) throw new TypeError("traceId must be non-empty");
    const id = options.id ?? `typescript-sdk-${this.#nextId++}`;
    if (typeof id !== "string" || id.length === 0) throw new TypeError("id must be a non-empty string");
    if (this.#pending.has(id)) throw new TypeError(`request id is already pending: ${id}`);

    const meta: RequestEnvelope<Params>["meta"] = {
      protocolVersion: PROTOCOL_VERSION,
      idempotencyKey: options.idempotencyKey,
      ...(options.deadline === undefined ? {} : { deadline: options.deadline }),
      ...(options.traceId === undefined ? {} : { traceId: options.traceId }),
    };
    const envelope: RequestEnvelope<Params> = { type: "request", id, method, params, meta };
    if (!isRequestEnvelope(envelope)) throw new TypeError("request does not satisfy jeden.session.v1");
    const { promise, resolve, reject } = Promise.withResolvers<JsonValue>();
    this.#pending.set(id, { resolve, reject });
    this.#start();
    try {
      await this.transport.send(envelope);
    } catch (error) {
      this.#pending.delete(id);
      reject(error);
    }
    return promise as Promise<Result>;
  }

  /** Sends canonical session.replay without rewriting returned cursors or sequences. */
  replay<Result extends JsonValue = JsonValue>(
    params: ReplayParams,
    options: RequestOptions,
  ): Promise<Result> {
    if (params.sessionId.length === 0) throw new TypeError("sessionId must be a non-empty string");
    if (params.cursor !== undefined && params.cursor.length === 0) {
      throw new TypeError("cursor must be a non-empty string when provided");
    }
    if (params.limit !== undefined && (!Number.isInteger(params.limit) || params.limit < 1)) {
      throw new TypeError("limit must be a positive integer when provided");
    }
    const jsonParams: JsonValue = {
      sessionId: params.sessionId,
      ...(params.cursor === undefined ? {} : { cursor: params.cursor }),
      ...(params.limit === undefined ? {} : { limit: params.limit }),
    };
    return this.request<Result>("session.replay", jsonParams, options);
  }

  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    const error = new TransportClosedError("The session client was closed");
    this.#rejectPending(error);
    this.#events.close();
    await this.transport.close?.();
  }

  #start(): void {
    if (this.#started) return;
    this.#started = true;
    void this.#receive();
  }

  async #receive(): Promise<void> {
    try {
      for await (const input of this.transport.receive()) {
        const envelope = parseEnvelope(input);
        switch (envelope.type) {
          case "event":
            this.#events.push(envelope);
            break;
          case "response": {
            const pending = this.#pending.get(envelope.id);
            if (pending === undefined) throw new EnvelopeValidationError(`Unexpected response id: ${envelope.id}`);
            this.#pending.delete(envelope.id);
            pending.resolve(envelope.result);
            break;
          }
          case "error": {
            if (envelope.id === undefined) throw new ProtocolError(envelope);
            const pending = this.#pending.get(envelope.id);
            if (pending === undefined) throw new EnvelopeValidationError(`Unexpected error id: ${envelope.id}`);
            this.#pending.delete(envelope.id);
            pending.reject(new ProtocolError(envelope));
            break;
          }
          case "request":
            throw new EnvelopeValidationError("Client transport received a request envelope");
        }
      }
      if (!this.#closed) this.#finish(new TransportClosedError());
    } catch (error) {
      this.#finish(error);
    }
  }

  #finish(error: unknown): void {
    this.#closed = true;
    this.#rejectPending(error);
    this.#events.fail(error);
  }

  #rejectPending(error: unknown): void {
    for (const pending of this.#pending.values()) pending.reject(error);
    this.#pending.clear();
  }
}
