import type { Envelope, RequestEnvelope } from "./types.js";
import { parseEnvelope } from "./validators.js";

export interface AsyncEnvelopeTransport {
  send(envelope: RequestEnvelope): Promise<void>;
  receive(): AsyncIterable<unknown>;
  close?(): Promise<void>;
}

export interface AsyncLineTransport {
  send(line: string): Promise<void>;
  receive(): AsyncIterable<string>;
  close?(): Promise<void>;
}

/** Adapts an injected newline-delimited JSON transport without owning any I/O. */
export class JsonLineTransport implements AsyncEnvelopeTransport {
  constructor(private readonly lines: AsyncLineTransport) {}

  async send(envelope: RequestEnvelope): Promise<void> {
    await this.lines.send(`${JSON.stringify(envelope)}\n`);
  }

  async *receive(): AsyncIterable<Envelope> {
    let buffered = "";
    for await (const chunk of this.lines.receive()) {
      buffered += chunk;
      for (;;) {
        const newline = buffered.indexOf("\n");
        if (newline < 0) break;
        const line = buffered.slice(0, newline).trim();
        buffered = buffered.slice(newline + 1);
        if (line.length > 0) yield parseEnvelope(JSON.parse(line) as unknown);
      }
    }
    const finalLine = buffered.trim();
    if (finalLine.length > 0) yield parseEnvelope(JSON.parse(finalLine) as unknown);
  }

  async close(): Promise<void> {
    await this.lines.close?.();
  }
}
