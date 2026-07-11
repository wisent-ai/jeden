import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { EventEmitter } from "node:events";
import { createInterface, type Interface } from "node:readline";
import { parseMessage, type JsonRpcMessage } from "./protocol.js";
import { publicMetadata, sensitiveMetadata, type RedactingLogger } from "./logging.js";

export interface AcpTransport {
  readonly events: EventEmitter;
  start(): Promise<void>;
  send(message: JsonRpcMessage): Promise<void>;
  close(): Promise<void>;
}

export interface SpawnOptions { readonly command: string; readonly args: readonly string[]; readonly cwd?: string; }

export class StdioAcpTransport implements AcpTransport {
  readonly events = new EventEmitter();
  private child: ChildProcessWithoutNullStreams | undefined;
  private lines: Interface | undefined;
  private intentionalClose = false;

  constructor(private readonly options: SpawnOptions, private readonly logger: RedactingLogger) {}

  async start(): Promise<void> {
    if (this.child) throw new Error("ACP transport is already running");
    this.intentionalClose = false;
    const child = spawn(this.options.command, [...this.options.args], {
      ...(this.options.cwd === undefined ? {} : { cwd: this.options.cwd }),
      stdio: ["pipe", "pipe", "pipe"],
      shell: false,
      windowsHide: true,
    });
    this.child = child;
    child.once("error", (error) => { this.logger.error("acp.spawn.error", error); this.events.emit("error", error); });
    child.once("exit", (code, signal) => {
      this.logger.event("acp.exit", { code: publicMetadata(code), signal: publicMetadata(signal), intentional: publicMetadata(this.intentionalClose) });
      this.child = undefined;
      this.events.emit("close", { intentional: this.intentionalClose, code, signal });
    });
    child.stderr.on("data", (chunk: Buffer) => this.logger.event("acp.stderr", { bytes: publicMetadata(chunk.byteLength) }));
    this.lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
    this.lines.on("line", (line) => {
      if (line.trim().length === 0) return;
      try { this.events.emit("message", parseMessage(JSON.parse(line) as unknown)); }
      catch (error) { this.logger.error("acp.parse.error", error); this.events.emit("error", error); }
    });
    await new Promise<void>((resolve, reject) => {
      const onSpawn = (): void => { child.off("error", onError); resolve(); };
      const onError = (error: Error): void => { child.off("spawn", onSpawn); reject(error); };
      child.once("spawn", onSpawn);
      child.once("error", onError);
    });
    this.logger.event("acp.spawn", { executable: sensitiveMetadata(), argumentCount: publicMetadata(this.options.args.length) });
  }

  async send(message: JsonRpcMessage): Promise<void> {
    const child = this.child;
    if (!child?.stdin.writable) throw new Error("ACP transport is not connected");
    const frame = `${JSON.stringify(message)}\n`;
    await new Promise<void>((resolve, reject) => child.stdin.write(frame, (error) => error ? reject(error) : resolve()));
    this.logger.event("acp.send", { method: publicMetadata("method" in message ? message.method : "response"), id: publicMetadata("id" in message ? message.id : null), bytes: publicMetadata(Buffer.byteLength(frame)) });
  }

  async close(): Promise<void> {
    this.intentionalClose = true;
    this.lines?.close();
    this.lines = undefined;
    const child = this.child;
    if (!child) return;
    this.child = undefined;
    child.stdin.end();
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => { child.kill(); resolve(); }, 1_000);
      timer.unref();
      child.once("exit", () => { clearTimeout(timer); resolve(); });
    });
  }
}
