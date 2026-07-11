export interface LogSink { appendLine(value: string): void; }

type PrimitiveMetadata = string | number | boolean | null;
export type LogMetadata =
  | { readonly visibility: "public"; readonly value: PrimitiveMetadata }
  | { readonly visibility: "sensitive" }
  | { readonly visibility: "omitted" };

export function publicMetadata(value: PrimitiveMetadata): LogMetadata { return { visibility: "public", value }; }
export function sensitiveMetadata(): LogMetadata { return { visibility: "sensitive" }; }
export function omittedMetadata(): LogMetadata { return { visibility: "omitted" }; }

/** Logs only explicitly classified, bounded metadata. */
export class RedactingLogger {
  constructor(private readonly sink: LogSink, private readonly enabled: () => boolean) {}
  event(label: string, metadata: Readonly<Record<string, LogMetadata>> = {}): void {
    if (!this.enabled()) return;
    const safe: Record<string, PrimitiveMetadata> = {};
    for (const [key, metadataValue] of Object.entries(metadata)) {
      if (metadataValue.visibility === "sensitive") safe[key] = "[REDACTED]";
      else if (metadataValue.visibility === "omitted") safe[key] = "[OMITTED]";
      else safe[key] = typeof metadataValue.value === "string" ? metadataValue.value.slice(0, 120) : metadataValue.value;
    }
    this.sink.appendLine(`${new Date().toISOString()} ${label} ${JSON.stringify(safe)}`);
  }
  error(label: string, error: unknown): void { this.event(label, { errorType: publicMetadata(error instanceof Error ? error.name : typeof error) }); }
}
