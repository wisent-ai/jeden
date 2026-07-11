import * as vscode from "vscode";
import { randomBytes } from "node:crypto";
import { JedenAcpClient, type ClientInteraction } from "./client.js";
import { RedactingLogger } from "./logging.js";
import { ExtensionModel, type Artifact, type DiagnosticRecord, type Job, type PendingAction } from "./model.js";
import type { InputRequest, PermissionRequest, SessionEventEnvelope } from "./protocol.js";
import { StdioAcpTransport } from "./transport.js";

class ModelTreeProvider<T extends { readonly id: string }> implements vscode.TreeDataProvider<T> {
  private readonly changed = new vscode.EventEmitter<T | undefined | void>();
  readonly onDidChangeTreeData = this.changed.event;
  constructor(private readonly source: () => Iterable<T>, private readonly item: (value: T) => vscode.TreeItem) {}
  refresh(): void { this.changed.fire(); }
  getTreeItem(element: T): vscode.TreeItem { return this.item(element); }
  getChildren(): T[] { return [...this.source()]; }
}

class DiffContentProvider implements vscode.TextDocumentContentProvider {
  private readonly changed = new vscode.EventEmitter<vscode.Uri>();
  readonly onDidChange = this.changed.event;
  constructor(private readonly model: ExtensionModel) {}
  provideTextDocumentContent(uri: vscode.Uri): string {
    const query = new URLSearchParams(uri.query);
    const action = this.model.pending.get(query.get("id") ?? "");
    return action?.kind === "diff" ? (query.get("side") === "after" ? action.after ?? "" : action.before ?? "") : "";
  }
}

class VscodeInteraction implements ClientInteraction {
  private readonly resolutions = new Map<string, (value: string | undefined) => void>();
  constructor(private readonly model: ExtensionModel) {}

  async requestPermission(request: PermissionRequest): Promise<string | undefined> {
    const id = typeof request.toolCall.toolCallId === "string" ? request.toolCall.toolCallId : `permission-${Date.now()}`;
    const title = typeof request.toolCall.title === "string" ? request.toolCall.title : "Jeden tool permission";
    this.model.pending.set(id, { id, kind: "approval", title }); this.model.emit("change");
    const commandResult = new Promise<string | undefined>((resolve) => this.resolutions.set(id, resolve));
    const labels = request.options.map((option) => ({ title: option.name, optionId: option.optionId }));
    const modalResult = vscode.window.showInformationMessage(title, { modal: true, detail: "Review the pending action before allowing it." }, ...labels.map((option) => option.title));
    const selected = await Promise.race([commandResult, modalResult.then((label) => labels.find((option) => option.title === label)?.optionId)]);
    this.resolutions.delete(id); this.model.pending.delete(id); this.model.emit("change"); return selected;
  }

  async requestInput(request: InputRequest): Promise<string | undefined> {
    const id = `input-${Date.now()}`;
    this.model.pending.set(id, { id, kind: "input", title: request.prompt });
    this.model.emit("change");
    const commandResult = new Promise<string | undefined>((resolve) => this.resolutions.set(id, resolve));
    const inputResult = vscode.window.showInputBox({ title: "Jeden input", prompt: request.prompt, ...(request.placeholder === undefined ? {} : { placeHolder: request.placeholder }), ...(request.password === undefined ? {} : { password: request.password }), ignoreFocusOut: true });
    try { return await Promise.race([commandResult, inputResult]); }
    finally { this.resolutions.delete(id); this.model.pending.delete(id); this.model.emit("change"); }
  }

  resolve(id: string, value: string | undefined): boolean { const resolve = this.resolutions.get(id); if (!resolve) return false; resolve(value); return true; }
  cancelPending(): void { for (const resolve of this.resolutions.values()) resolve(undefined); this.resolutions.clear(); }
}

class ChatViewProvider implements vscode.WebviewViewProvider {
  private view: vscode.WebviewView | undefined;
  constructor(private readonly model: ExtensionModel, private readonly run: (command: "connect" | "new" | "prompt" | "cancel", value?: string) => Promise<void>) {}
  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view; view.webview.options = { enableScripts: true };
    const nonce = randomBytes(16).toString("base64");
    view.webview.html = `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'nonce-${nonce}'; script-src 'nonce-${nonce}'"><style nonce="${nonce}">body{font-family:var(--vscode-font-family);padding:8px}.messages{white-space:pre-wrap}.item{margin:8px 0;padding:6px;border-left:2px solid var(--vscode-focusBorder)}textarea{width:100%;box-sizing:border-box;resize:vertical}button{margin:6px 6px 0 0}.status{color:var(--vscode-descriptionForeground)}</style></head><body><div id="status" class="status"></div><div id="messages" class="messages"></div><textarea id="prompt" rows="4" aria-label="Prompt"></textarea><div><button id="send">Send</button><button id="cancel">Cancel</button><button id="new">New session</button><button id="connect">Connect</button></div><script nonce="${nonce}">const vscode=acquireVsCodeApi(),messages=document.getElementById('messages'),status=document.getElementById('status'),prompt=document.getElementById('prompt');addEventListener('message',({data})=>{status.textContent=data.status;messages.replaceChildren(...data.items.map(item=>{const el=document.createElement('div');el.className='item';el.textContent=item.role+': '+item.text;return el}))});document.getElementById('send').onclick=()=>{vscode.postMessage({command:'prompt',value:prompt.value});prompt.value=''};for(const id of ['cancel','new','connect'])document.getElementById(id).onclick=()=>vscode.postMessage({command:id});</script></body></html>`;
    view.webview.onDidReceiveMessage((message: unknown) => { if (typeof message !== "object" || message === null) return; const value = message as { command?: unknown; value?: unknown }; if (value.command === "connect" || value.command === "new" || value.command === "cancel") void this.run(value.command); else if (value.command === "prompt" && typeof value.value === "string") void this.run("prompt", value.value); });
    this.refresh();
  }
  refresh(): void { void this.view?.webview.postMessage({ status: `${this.model.serviceStatus} · ${this.model.modelStatus} · ${this.model.accountStatus}`, items: this.model.transcript }); }
}

function treeItem(label: string, description: string, contextValue: string, command?: vscode.Command): vscode.TreeItem {
  const item = new vscode.TreeItem(label); item.description = description; item.contextValue = contextValue; if (command) item.command = command; return item;
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const model = new ExtensionModel();
  const output = vscode.window.createOutputChannel("Jeden ACP"); context.subscriptions.push(output);
  const logger = new RedactingLogger(output, () => vscode.workspace.getConfiguration("jeden.acp").get<boolean>("trace", false));
  const interaction = new VscodeInteraction(model);
  const workspace = (): string | undefined => vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const client = new JedenAcpClient({
    transport: () => { const config = vscode.workspace.getConfiguration("jeden.acp"); const cwd = workspace(); return new StdioAcpTransport({ command: config.get<string>("command", "jeden"), args: config.get<string[]>("arguments", ["acp"]), ...(cwd === undefined ? {} : { cwd }) }, logger); },
    interaction, logger,
    autoReconnect: () => vscode.workspace.getConfiguration("jeden.acp").get<boolean>("autoReconnect", true),
    reconnectLimit: () => vscode.workspace.getConfiguration("jeden.acp").get<number>("reconnectLimit", 5),
  });

  const diagnostics = vscode.languages.createDiagnosticCollection("jeden"); context.subscriptions.push(diagnostics);
  const pendingProvider = new ModelTreeProvider(() => model.pending.values(), (value: PendingAction) => treeItem(value.title, value.kind, value.kind === "diff" ? "jeden.diff" : "jeden.pending"));
  const artifactProvider = new ModelTreeProvider(() => model.artifacts.values(), (value: Artifact) => treeItem(value.name, value.mediaType ?? "artifact", "jeden.artifact", { command: "jeden.openArtifact", title: "Open", arguments: [value] }));
  const jobProvider = new ModelTreeProvider(() => model.jobs.values(), (value: Job) => treeItem(value.label, value.state, "jeden.job"));
  const diagnosticProvider = new ModelTreeProvider(() => model.diagnostics.values(), (value: DiagnosticRecord) => treeItem(value.message, value.severity, "jeden.diagnostic", { command: "vscode.open", title: "Open", arguments: [vscode.Uri.parse(value.uri)] }));
  context.subscriptions.push(vscode.window.registerTreeDataProvider("jeden.pending", pendingProvider), vscode.window.registerTreeDataProvider("jeden.artifacts", artifactProvider), vscode.window.registerTreeDataProvider("jeden.jobs", jobProvider), vscode.window.registerTreeDataProvider("jeden.diagnostics", diagnosticProvider));
  const diffProvider = new DiffContentProvider(model); context.subscriptions.push(vscode.workspace.registerTextDocumentContentProvider("jeden-diff", diffProvider));
  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50); status.name = "Jeden"; status.command = "jeden.connect"; status.show(); context.subscriptions.push(status);

  const updateContexts = async (): Promise<void> => {
    const connected = client.capabilities !== undefined;
    await Promise.all([
      vscode.commands.executeCommand("setContext", "jeden.connected", connected),
      vscode.commands.executeCommand("setContext", "jeden.sessionActive", client.session !== undefined),
      vscode.commands.executeCommand("setContext", "jeden.turnActive", client.turnActive),
      vscode.commands.executeCommand("setContext", "jeden.pendingAction", model.pending.size > 0),
      ...(["sessionNew", "sessionLoad", "prompt", "cancel"] as const).map((capability) => vscode.commands.executeCommand("setContext", `jeden.capability.${capability}`, client.hasCapability(capability))),
    ]);
  };
  const connect = async (): Promise<void> => {
    model.serviceStatus = "Connecting";
    model.emit("change");
    try { await client.connect(); model.serviceStatus = "Connected"; }
    catch (error) { model.serviceStatus = "Disconnected"; model.emit("change"); throw error; }
    await updateContexts();
  };
  const newSession = async (): Promise<void> => { client.requireCapability("sessionNew"); const cwd = workspace(); if (!cwd) throw new Error("Open a workspace before creating a Jeden session"); model.clearSession(); await client.newSession(cwd); await context.workspaceState.update("jeden.session", client.session); await updateContexts(); };
  const prompt = async (provided?: string): Promise<void> => { client.requireCapability("prompt"); const value = provided ?? await vscode.window.showInputBox({ title: "Prompt Jeden", ignoreFocusOut: true }); if (!value) return; model.transcript.push({ role: "user", text: value }); model.emit("change"); await client.prompt(value); };
  const runSafely = (operation: () => Promise<void>): void => { void operation().catch((error: unknown) => { logger.error("command.error", error); void vscode.window.showErrorMessage(error instanceof Error ? error.message : "Jeden command failed"); }); };
  const chat = new ChatViewProvider(model, async (command, value) => { if (command === "connect") await connect(); else if (command === "new") await newSession(); else if (command === "prompt") await prompt(value); else await client.cancel(); });
  context.subscriptions.push(vscode.window.registerWebviewViewProvider("jeden.chat", chat));

  const selectPending = async (argument: PendingAction | undefined): Promise<PendingAction | undefined> => argument ?? (model.pending.size === 1 ? [...model.pending.values()][0] : await vscode.window.showQuickPick([...model.pending.values()].map((item) => ({ label: item.title, item }))).then((picked) => picked?.item));
  const approve = async (argument?: PendingAction): Promise<void> => {
    const action = await selectPending(argument); if (!action) return;
    if (action.kind === "approval") { interaction.resolve(action.id, "allow-once"); return; }
    if (action.kind !== "diff" || !action.uri) return;
    const uri = vscode.Uri.parse(action.uri); const document = await vscode.workspace.openTextDocument(uri); const current = document.getText();
    if (current !== action.before) throw new Error("Pending diff is stale; the document changed after it was proposed");
    const last = document.lineAt(document.lineCount - 1).range.end; const edit = new vscode.WorkspaceEdit(); edit.replace(uri, new vscode.Range(new vscode.Position(0, 0), last), action.after ?? "");
    if (!await vscode.workspace.applyEdit(edit)) throw new Error("VS Code rejected the pending diff");
    model.pending.delete(action.id); model.emit("change");
  };
  const reject = async (argument?: PendingAction): Promise<void> => { const action = await selectPending(argument); if (!action) return; if (action.kind === "approval") interaction.resolve(action.id, undefined); model.pending.delete(action.id); model.emit("change"); };
  const openDiff = async (argument?: PendingAction): Promise<void> => { const action = await selectPending(argument); if (!action || action.kind !== "diff") return; const before = vscode.Uri.from({ scheme: "jeden-diff", path: `/${encodeURIComponent(action.title)}.before`, query: new URLSearchParams({ id: action.id, side: "before" }).toString() }); const after = vscode.Uri.from({ scheme: "jeden-diff", path: `/${encodeURIComponent(action.title)}.after`, query: new URLSearchParams({ id: action.id, side: "after" }).toString() }); await vscode.commands.executeCommand("vscode.diff", before, after, action.title); };
  const openArtifact = async (artifact?: Artifact): Promise<void> => { if (!artifact) return; const uri = vscode.Uri.parse(artifact.uri); if (uri.scheme === "file" || uri.scheme === "untitled") await vscode.window.showTextDocument(uri); else if (uri.scheme === "https") await vscode.env.openExternal(uri); else throw new Error(`Unsupported artifact URI scheme: ${uri.scheme}`); };
  const loadSession = async (): Promise<void> => { client.requireCapability("sessionLoad"); const cwd = workspace(); if (!cwd) throw new Error("Open a workspace before loading a session"); const sessionId = await vscode.window.showInputBox({ title: "Load Jeden session", prompt: "Session ID or durable session path", ignoreFocusOut: true }); if (!sessionId) return; model.clearSession(); await client.loadSession(sessionId, cwd); await context.workspaceState.update("jeden.session", client.session); await updateContexts(); };

  const handlers: ReadonlyArray<[string, (...args: unknown[]) => void]> = [
    ["jeden.connect", () => runSafely(connect)], ["jeden.disconnect", () => runSafely(async () => { await client.dispose(); model.serviceStatus = "Disconnected"; await updateContexts(); })],
    ["jeden.newSession", () => runSafely(newSession)], ["jeden.loadSession", () => runSafely(loadSession)], ["jeden.prompt", () => runSafely(() => prompt())], ["jeden.cancel", () => runSafely(() => client.cancel())],
    ["jeden.approvePending", (value) => runSafely(() => approve(value as PendingAction | undefined))], ["jeden.rejectPending", (value) => runSafely(() => reject(value as PendingAction | undefined))],
    ["jeden.openArtifact", (value) => runSafely(() => openArtifact(value as Artifact | undefined))], ["jeden.openDiff", (value) => runSafely(() => openDiff(value as PendingAction | undefined))],
    ["jeden.refresh", () => { pendingProvider.refresh(); artifactProvider.refresh(); jobProvider.refresh(); diagnosticProvider.refresh(); chat.refresh(); }],
  ];
  for (const [id, handler] of handlers) context.subscriptions.push(vscode.commands.registerCommand(id, handler));

  model.on("change", () => {
    pendingProvider.refresh(); artifactProvider.refresh(); jobProvider.refresh(); diagnosticProvider.refresh(); chat.refresh(); status.text = `$(sparkle) Jeden: ${model.serviceStatus}`; status.tooltip = `${model.modelStatus} · ${model.accountStatus}`;
    const byUri = new Map<string, vscode.Diagnostic[]>();
    for (const record of model.diagnostics.values()) { const severity = record.severity === "error" ? vscode.DiagnosticSeverity.Error : record.severity === "info" ? vscode.DiagnosticSeverity.Information : vscode.DiagnosticSeverity.Warning; const diagnostic = new vscode.Diagnostic(new vscode.Range(record.line, record.character, record.line, record.character + 1), record.message, severity); const values = byUri.get(record.uri) ?? []; values.push(diagnostic); byUri.set(record.uri, values); }
    diagnostics.clear(); for (const [uri, values] of byUri) diagnostics.set(vscode.Uri.parse(uri), values); void updateContexts();
  });
  client.on("event", (event: SessionEventEnvelope) => model.apply(event));
  client.on("connected", () => { model.serviceStatus = "Connected"; model.emit("change"); });
  client.on("disconnected", () => { model.serviceStatus = "Disconnected"; model.emit("change"); });
  client.on("resumed", () => { model.serviceStatus = "Reconnected"; model.emit("change"); });
  client.on("turn", () => { void updateContexts(); });
  client.on("error", (error: Error) => logger.error("acp.error", error));
  context.subscriptions.push({ dispose: () => { void client.dispose(); model.removeAllListeners(); } });
  await updateContexts();
}

export function deactivate(): void {}
