#!/usr/bin/env node

import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

const argv = process.argv.slice(2);
const option = (name, fallback = undefined) => {
  const index = argv.indexOf(name);
  return index >= 0 && index + 1 < argv.length ? argv[index + 1] : fallback;
};
const flag = name => argv.includes(name);
const session = option("--session", "default").replace(/[^a-zA-Z0-9_.-]/g, "_");
const chrome = option("--chrome");
const visible = flag("--visible");
const sessionHash = [...session].reduce((hash, character) => ((hash * 33) ^ character.charCodeAt(0)) >>> 0, 5381);
const port = Number(option("--port", String(9300 + (sessionHash % 300))));
const root = join(option("--state-dir", join(process.cwd(), ".jeden", "browser")), session);
const profile = option("--user-data-dir", join(root, "profile"));
const statePath = join(root, "state.json");
const endpoint = `http://127.0.0.1:${port}`;

const sleep = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
const readStdin = async () => {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  const text = Buffer.concat(chunks).toString("utf8").trim();
  return text ? JSON.parse(text) : {};
};
const readState = async () => {
  try {
    return JSON.parse(await readFile(statePath, "utf8"));
  } catch {
    return {};
  }
};
const saveState = async state => {
  await mkdir(root, { recursive: true });
  await writeFile(statePath, `${JSON.stringify(state)}\n`, { mode: 0o600 });
};
const requestJson = async (path, init = undefined) => {
  const response = await fetch(`${endpoint}${path}`, init);
  if (!response.ok) throw new Error(`CDP HTTP ${response.status}: ${await response.text()}`);
  return response.json();
};
const browserReady = async () => {
  try {
    const version = await requestJson("/json/version");
    return typeof version.webSocketDebuggerUrl === "string";
  } catch {
    return false;
  }
};
const ensureBrowser = async () => {
  if (await browserReady()) return;
  if (!chrome) throw new Error("Chromium executable is not configured");
  await mkdir(profile, { recursive: true });
  const browserTmp = join(root, "tmp");
  await mkdir(browserTmp, { recursive: true });
  const args = [
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    "--remote-allow-origins=*",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-background-networking",
    "--disable-component-update",
    // Chromium cannot initialize a nested Seatbelt profile after inheriting Jeden's
    // outer process sandbox. The outer profile remains enforced for every child.
    "--no-sandbox",
    "--disable-breakpad",
    "--disable-crash-reporter",
  ];
  if (!visible) args.push("--headless=new", "--disable-gpu");
  args.push("about:blank");
  const child = spawn(chrome, args, {
    detached: true,
    stdio: "ignore",
    env: { ...process.env, TMPDIR: browserTmp },
  });
  const state = await readState();
  state.browserPid = child.pid;
  await saveState(state);
  child.unref();
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (await browserReady()) return;
    await sleep(100);
  }
  throw new Error(`Chromium did not expose CDP on ${endpoint}`);
};
const listTabs = async () => (await requestJson("/json/list"))
  .filter(target => target.type === "page")
  .map(target => ({ id: target.id, title: target.title, url: target.url, type: target.type }));
const resolveTab = async (input, state) => {
  const tabs = await listTabs();
  const requested = input.tab ?? input.targetId ?? state.currentTab;
  return tabs.find(tab => tab.id === requested) ?? tabs[0] ?? null;
};
const openTab = async url => {
  const encoded = encodeURIComponent(url || "about:blank");
  const target = await requestJson(`/json/new?${encoded}`, { method: "PUT" });
  return { id: target.id, title: target.title, url: target.url, type: target.type };
};
const activateTab = async id => {
  await requestJson(`/json/activate/${encodeURIComponent(id)}`);
};
const closeTab = async id => {
  await requestJson(`/json/close/${encodeURIComponent(id)}`);
};

class CdpClient {
  constructor(url) {
    this.url = url;
    this.socket = null;
    this.sequence = 1;
    this.pending = new Map();
  }
  async connect() {
    this.socket = new WebSocket(this.url);
    this.socket.addEventListener("message", event => {
      const message = JSON.parse(String(event.data));
      if (!message.id) return;
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message ?? JSON.stringify(message.error)));
      else pending.resolve(message.result ?? {});
    });
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("CDP WebSocket connection timed out")), 5000);
      this.socket.addEventListener("open", () => { clearTimeout(timer); resolve(); }, { once: true });
      this.socket.addEventListener("error", () => { clearTimeout(timer); reject(new Error("CDP WebSocket connection failed")); }, { once: true });
    });
  }
  send(method, params = {}) {
    const id = this.sequence++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, 30000);
      this.pending.set(id, {
        resolve: value => { clearTimeout(timer); resolve(value); },
        reject: error => { clearTimeout(timer); reject(error); },
      });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
  close() {
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.close();
  }
}

const evaluate = async (client, expression, awaitPromise = true) => {
  const result = await client.send("Runtime.evaluate", {
    expression,
    awaitPromise,
    returnByValue: true,
    userGesture: true,
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text ?? "evaluation failed");
  }
  return result.result?.value;
};
const jsString = value => JSON.stringify(String(value));
const waitReady = async (client, timeout) => {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const ready = await evaluate(client, "document.readyState");
    if (ready === "complete" || ready === "interactive") return;
    await sleep(50);
  }
  throw new Error(`page readiness timed out after ${timeout}ms`);
};
const waitSelector = async (client, selector, timeout) => {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const found = await evaluate(client, `Boolean(document.querySelector(${jsString(selector)}))`);
    if (found) return;
    await sleep(50);
  }
  throw new Error(`selector ${selector} timed out after ${timeout}ms`);
};
const pageSnapshotExpression = `(() => {
  const visible = element => {
    const style = getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return style.visibility !== "hidden" && style.display !== "none" && rect.width > 0 && rect.height > 0;
  };
  const elements = [...document.querySelectorAll("a,button,input,textarea,select,[role],[contenteditable=true]")]
    .filter(visible).slice(0, 200).map((element, index) => ({
      index,
      tag: element.tagName.toLowerCase(),
      role: element.getAttribute("role"),
      text: (element.innerText || element.value || element.getAttribute("aria-label") || element.getAttribute("title") || "").trim().slice(0, 300),
      id: element.id || null,
      name: element.getAttribute("name"),
      type: element.getAttribute("type"),
      href: element.href || null,
      disabled: Boolean(element.disabled),
    }));
  return { title: document.title, url: location.href, text: (document.body?.innerText || "").slice(0, 20000), elements };
})()`;

const handleTabAction = async (action, input, state) => {
  if (action === "list") return { ok: true, action, tabs: await listTabs(), currentTab: state.currentTab ?? null };
  if (action === "open" || action === "new") {
    const tab = await openTab(input.url ?? "about:blank");
    state.currentTab = tab.id;
    await saveState(state);
    return { ok: true, action: "open", tab };
  }
  const tab = await resolveTab(input, state);
  if (!tab) throw new Error("no browser tab is available");
  if (action === "focus" || action === "activate") {
    await activateTab(tab.id);
    state.currentTab = tab.id;
    await saveState(state);
    return { ok: true, action: "focus", tab };
  }
  if (action === "close") {
    await closeTab(tab.id);
    if (state.currentTab === tab.id) delete state.currentTab;
    await saveState(state);
    return { ok: true, action, tab };
  }
  throw new Error(`unsupported browser_tab action: ${action}`);
};

const handlePageAction = async (action, input, state) => {
  let tab = await resolveTab(input, state);
  if (!tab) tab = await openTab("about:blank");
  state.currentTab = tab.id;
  await saveState(state);
  const targets = await requestJson("/json/list");
  const target = targets.find(item => item.id === tab.id);
  if (!target?.webSocketDebuggerUrl) throw new Error(`tab ${tab.id} has no CDP endpoint`);
  const client = new CdpClient(target.webSocketDebuggerUrl);
  await client.connect();
  try {
    await client.send("Page.enable");
    await client.send("Runtime.enable");
    const timeout = Math.max(1, Math.min(Number(input.timeout ?? 30000), 60000));
    let value;
    switch (action) {
      case "navigate":
      case "goto": {
        const url = String(input.url ?? "").trim();
        if (!url) throw new Error("url is required");
        await client.send("Page.navigate", { url });
        await waitReady(client, timeout);
        value = { url: await evaluate(client, "location.href"), title: await evaluate(client, "document.title") };
        break;
      }
      case "click": {
        if (input.selector) {
          value = await evaluate(client, `(() => { const element = document.querySelector(${jsString(input.selector)}); if (!element) throw new Error("selector not found"); element.click(); return true; })()`);
        } else {
          const x = Number(input.x);
          const y = Number(input.y);
          if (!Number.isFinite(x) || !Number.isFinite(y)) throw new Error("selector or finite x/y coordinates are required");
          await client.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
          await client.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 });
          value = true;
        }
        break;
      }
      case "type":
      case "fill": {
        const selector = String(input.selector ?? "").trim();
        if (!selector) throw new Error("selector is required");
        const text = String(input.text ?? input.value ?? "");
        value = await evaluate(client, `(() => { const element = document.querySelector(${jsString(selector)}); if (!element) throw new Error("selector not found"); element.focus(); element.value = ${jsString(text)}; element.dispatchEvent(new Event("input", { bubbles: true })); element.dispatchEvent(new Event("change", { bubbles: true })); return element.value; })()`);
        break;
      }
      case "press": {
        const key = String(input.key ?? "Enter");
        await client.send("Input.dispatchKeyEvent", { type: "keyDown", key });
        await client.send("Input.dispatchKeyEvent", { type: "keyUp", key });
        value = true;
        break;
      }
      case "evaluate":
      case "eval": {
        const expression = String(input.expression ?? input.code ?? "").trim();
        if (!expression) throw new Error("expression is required");
        value = await evaluate(client, expression, true);
        break;
      }
      case "wait": {
        if (input.selector) await waitSelector(client, String(input.selector), timeout);
        else await sleep(Math.max(0, Math.min(Number(input.ms ?? input.milliseconds ?? 250), timeout)));
        value = true;
        break;
      }
      case "scroll": {
        const x = Number(input.x ?? input.deltaX ?? 0);
        const y = Number(input.y ?? input.deltaY ?? 600);
        value = await evaluate(client, `(() => { scrollBy(${Number.isFinite(x) ? x : 0}, ${Number.isFinite(y) ? y : 600}); return { x: scrollX, y: scrollY }; })()`);
        break;
      }
      case "inspect":
      case "observe":
      case "snapshot": {
        value = await evaluate(client, pageSnapshotExpression);
        break;
      }
      case "screenshot": {
        const format = input.format === "jpeg" ? "jpeg" : "png";
        const params = { format, captureBeyondViewport: input.fullPage !== false };
        if (format === "jpeg") params.quality = Math.max(0, Math.min(Number(input.quality ?? 85), 100));
        const capture = await client.send("Page.captureScreenshot", params);
        return { ok: true, action, tab: tab.id, format, data: capture.data };
      }
      default:
        throw new Error(`unsupported browser action: ${action}`);
    }
    return { ok: true, action, tab: tab.id, value };
  } finally {
    client.close();
  }
};

try {
  const request = await readStdin();
  const input = request.input && typeof request.input === "object" ? request.input : request;
  const action = String(request.action ?? input.action ?? "").trim().toLowerCase();
  if (!action) throw new Error("action is required");
  await ensureBrowser();
  const state = await readState();
  const tool = String(input.tool ?? request.tool ?? "");
  const result = tool === "browser_tab" || ["list", "open", "new", "focus", "activate", "close"].includes(action)
    ? await handleTabAction(action, input, state)
    : await handlePageAction(action, input, state);
  const latestState = await readState();
  if (Number.isInteger(latestState.browserPid)) result.browserPid = latestState.browserPid;
  process.stdout.write(JSON.stringify(result));
} catch (error) {
  process.stdout.write(JSON.stringify({ ok: false, error: error instanceof Error ? error.message : String(error) }));
}
