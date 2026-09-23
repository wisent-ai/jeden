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
// A page loads when it loads and an element appears when the page renders it;
// the turn's own cancellation is what ends a wait that should not continue.
const waitReady = async client => {
  for (;;) {
    const ready = await evaluate(client, "document.readyState");
    if (ready === "complete" || ready === "interactive") return;
    await sleep(50);
  }
};
const waitSelector = async (client, selector) => {
  for (;;) {
    const found = await evaluate(client, `Boolean(document.querySelector(${jsString(selector)}))`);
    if (found) return;
    await sleep(50);
  }
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
