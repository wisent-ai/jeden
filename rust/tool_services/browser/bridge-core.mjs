#!/usr/bin/env node

import { spawn } from "node:child_process";
import { watch } from "node:fs";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
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
// Chromium writes DevToolsActivePort into the profile once CDP listens. The
// directory watch reports that write, and the child's exit is the failure;
// there is no retry loop and no deadline (cli.md rule 8).
const ensureBrowser = async () => {
  if (await browserReady()) return;
  if (!chrome) throw new Error("Chromium executable is not configured");
  await mkdir(profile, { recursive: true });
  const activePort = join(profile, "DevToolsActivePort");
  await rm(activePort, { force: true });
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
  const watcher = watch(profile);
  const listening = new Promise((resolve, reject) => {
    watcher.on("change", (_, name) => {
      if (String(name) === "DevToolsActivePort") resolve();
    });
    watcher.on("error", reject);
  });
  const child = spawn(chrome, args, {
    detached: true,
    stdio: "ignore",
    env: { ...process.env, TMPDIR: browserTmp },
  });
  const exited = new Promise((_, reject) => {
    child.once("error", error => reject(new Error(`Chromium could not start: ${error.message}`)));
    child.once("exit", (code, signal) => reject(new Error(
      `Chromium exited (code ${code ?? "none"}, signal ${signal ?? "none"}) before exposing CDP on ${endpoint}`,
    )));
  });
  const state = await readState();
  state.browserPid = child.pid;
  await saveState(state);
  try {
    await Promise.race([listening, exited]);
  } finally {
    watcher.close();
    child.removeAllListeners("exit");
    child.removeAllListeners("error");
    child.unref();
  }
  if (!(await browserReady())) throw new Error(`Chromium wrote DevToolsActivePort but CDP on ${endpoint} did not answer`);
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
    this.events = new Map();
  }
  async connect() {
    this.socket = new WebSocket(this.url);
    this.socket.addEventListener("message", event => {
      const message = JSON.parse(String(event.data));
      if (!message.id) {
        const listeners = this.events.get(message.method);
        if (!listeners) return;
        this.events.delete(message.method);
        for (const listener of listeners) listener.resolve(message.params ?? {});
        return;
      }
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message ?? JSON.stringify(message.error)));
      else pending.resolve(message.result ?? {});
    });
    // A closed socket fails every call and event still outstanding on it.
    this.socket.addEventListener("close", () => {
      const closed = new Error("CDP WebSocket closed");
      for (const pending of this.pending.values()) pending.reject(closed);
      this.pending.clear();
      for (const listeners of this.events.values()) for (const listener of listeners) listener.reject(closed);
      this.events.clear();
    });
    await new Promise((resolve, reject) => {
      this.socket.addEventListener("open", () => resolve(), { once: true });
      this.socket.addEventListener("error", () => reject(new Error("CDP WebSocket connection failed")), { once: true });
    });
  }
  send(method, params = {}) {
    const id = this.sequence++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
  // Resolves with the params of the next `method` event on this target.
  next(method) {
    return new Promise((resolve, reject) => {
      const listeners = this.events.get(method) ?? [];
      listeners.push({ resolve, reject });
      this.events.set(method, listeners);
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
// An element appears when the page renders it: the page's own MutationObserver
// reports it, and the turn's own cancellation ends a request that should stop.
const untilSelector = (client, selector) => evaluate(client, `new Promise(resolve => {
  const selector = ${jsString(selector)};
  if (document.querySelector(selector)) { resolve(true); return; }
  const observer = new MutationObserver(() => {
    if (document.querySelector(selector)) { observer.disconnect(); resolve(true); }
  });
  observer.observe(document, { childList: true, subtree: true, attributes: true });
})`);
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
