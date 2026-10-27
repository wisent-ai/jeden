#!/usr/bin/env node

import { spawn } from "node:child_process";
import { constants, existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { inspect } from "node:util";

const argv = process.argv;
/** The value after `name` on the command line, or undefined when `name` is not there. */
const option = name => {
  if (!argv.includes(name)) return undefined;
  const [, value] = argv.slice(argv.indexOf(name));
  if (value === undefined) throw new Error(`${name} is given without its value`);
  return value;
};
const flag = name => argv.includes(name);
const named = option("--session");
if (named === undefined) throw new Error("--session is required: the browser service names the session every bridge call belongs to");
const session = named.replace(/[^\w.-]/g, "_");
const chrome = option("--chrome");
const visible = flag("--visible");
const root = join(process.cwd(), ".jeden", "browser", session);
const statedProfile = option("--user-data-dir");
const profile = statedProfile === undefined ? join(root, "profile") : statedProfile;
const statePath = join(root, "state.json");
// The CDP endpoint of this session's browser, as Chromium announced it when it started.
let endpoint;

const readStdin = async () => {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  const text = Buffer.concat(chunks).toString("utf8").trim();
  if (!text) throw new Error("the bridge was given no request on stdin");
  return JSON.parse(text);
};
// A session that never started a browser has no state yet.
const readState = async () => {
  if (!existsSync(statePath)) return {};
  try {
    return JSON.parse(await readFile(statePath, "utf8"));
  } catch (error) {
    throw new Error(`Browser state ${statePath} could not be read: ${inspect(error, {
      depth: null, maxArrayLength: null, maxStringLength: null,
    })}`, { cause: error });
  }
};
const saveState = async state => {
  await mkdir(root, { recursive: true });
  // Owner read and write only: the state names the session's browser process.
  await writeFile(statePath, `${JSON.stringify(state)}\n`, { mode: constants.S_IRUSR | constants.S_IWUSR });
};
const requestJson = async (path, init) => {
  const url = `${endpoint}${path}`;
  try {
    const response = await fetch(url, init);
    if (!response.ok) throw new Error(`HTTP ${response.status}: ${await response.text()}`);
    return await response.json();
  } catch (error) {
    throw new Error(`CDP request ${url} failed: ${inspect(error, {
      depth: null, maxArrayLength: null, maxStringLength: null,
    })}`);
  }
};
const unready = cause => ({ ready: false, cause });
/** Whether the browser the state recorded still answers CDP, and why not when it does not. */
const probe = async state => {
  if (typeof state.endpoint !== "string") return unready("this session has not started a browser");
  endpoint = state.endpoint;
  try {
    const version = await requestJson("/json/version");
    return { ready: typeof version.webSocketDebuggerUrl === "string", cause: "its /json/version names no webSocketDebuggerUrl" };
  } catch (error) {
    return unready(error instanceof Error ? error.message : String(error));
  }
};
// Chromium is started with a remote-debugging port the system chooses and
// announces "DevTools listening on ws://<host>:<port>/…" on stderr once CDP
// accepts connections; that announcement is the endpoint and the readiness
// event, and the child's exit, with the stderr it wrote, is the failure.
// There is no retry loop and no deadline (cli.md rule 8). It answers the
// session state that records the browser it found or started, so a later
// action failure still reports that browser.
const ensureBrowser = async () => {
  const state = await readState();
  const recorded = await probe(state);
  if (recorded.ready) return state;
  if (!chrome) throw new Error(`Chromium executable is not configured, and ${recorded.cause}`);
  await mkdir(profile, { recursive: true });
  const browserTmp = join(root, "tmp");
  await mkdir(browserTmp, { recursive: true });
  const args = [
    "--remote-debugging-port=0",
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
  const child = spawn(chrome, args, { detached: true, stdio: "pipe", env: { ...process.env, TMPDIR: browserTmp } });
  // Nothing is written to the browser and its stdout is not read, so both are
  // released; stderr carries the announcement and the failure cause.
  child.stdin.end();
  child.stdout.resume();
  let stderr = "";
  const listening = new Promise(resolve => {
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", chunk => {
      stderr += chunk;
      const announced = /DevTools listening on (?<socket>ws:\/\/\S+)/.exec(stderr);
      if (announced) resolve(new URL(announced.groups.socket));
    });
  });
  const exited = new Promise(resolve => {
    child.once("error", error => resolve(new Error(`Chromium could not start: ${error.message}`, { cause: error })));
    child.once("exit", (code, signal) => resolve(new Error(`Chromium exited (code ${code}, signal ${signal}) before exposing CDP`)));
  });
  try {
    state.browserPid = child.pid;
    await saveState(state);
    const outcome = await Promise.race([listening, exited]);
    if (outcome instanceof Error) throw outcome;
    state.endpoint = `http://${outcome.host}`;
    endpoint = state.endpoint;
    await saveState(state);
    const version = await requestJson("/json/version");
    if (typeof version.webSocketDebuggerUrl !== "string") {
      throw new Error(`CDP readiness at ${endpoint}/json/version returned no string webSocketDebuggerUrl`);
    }
    return state;
  } catch (error) {
    if (child.pid !== undefined && child.exitCode === null && child.signalCode === null) {
      try {
        process.kill(-child.pid, "SIGTERM");
      } catch (cleanup) {
        if (cleanup.code !== "ESRCH") {
          throw new Error(`${error.message}; stopping Chromium process group ${child.pid} failed: ${inspect(cleanup)}; stderr: ${stderr}`, { cause: error });
        }
      }
      await exited;
    }
    throw new Error(`${error.message}; stderr: ${stderr}`, { cause: error });
  } finally {
    child.removeAllListeners("exit");
    child.removeAllListeners("error");
    // Later output is drained without holding this process open, so the
    // detached browser outlives the bridge call that started it.
    child.stderr.removeAllListeners("data");
    child.stderr.resume();
    child.stdout.unref();
    child.stderr.unref();
    child.unref();
  }
};
const listTabs = async () => (await requestJson("/json/list"))
  .filter(target => target.type === "page")
  .map(target => ({ id: target.id, title: target.title, url: target.url, type: target.type }));
/** The tab the caller named as `tab`, else the session's current tab, else its first; undefined when none is open. */
const resolveTab = async (input, state) => {
  const tabs = await listTabs();
  if (input.tab !== undefined) {
    const tab = tabs.find(candidate => candidate.id === input.tab);
    if (!tab) throw new Error(`no open tab ${input.tab}`);
    return tab;
  }
  const current = tabs.find(tab => tab.id === state.currentTab);
  if (current) return current;
  const [first] = tabs;
  return first;
};
const openTab = async url => {
  const target = await requestJson(`/json/new?${encodeURIComponent(url)}`, { method: "PUT" });
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
    // Every call's method in the order it was sent; a call's id is its place here.
    this.issued = [];
    this.pending = new Map();
    this.events = new Map();
    // The error the socket closed with; a call or event subscription made
    // after that is refused with it instead of waiting on a dead socket.
    this.closed = null;
  }
  async connect() {
    this.socket = new WebSocket(this.url);
    this.socket.addEventListener("message", event => {
      const message = JSON.parse(String(event.data));
      if (message.id === undefined) {
        const listeners = this.events.get(message.method);
        if (!listeners) return;
        this.events.delete(message.method);
        for (const listener of listeners) listener.resolve(message.params);
        return;
      }
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(`CDP ${pending.method} failed: ${JSON.stringify(message.error)}`));
      else pending.resolve(message.result);
    });
    // A closed socket fails every call and event still outstanding on it.
    this.socket.addEventListener("close", event => {
      const closed = new Error(`CDP WebSocket ${this.url} closed: code=${event.code}, reason=${JSON.stringify(event.reason)}, clean=${event.wasClean}`);
      this.closed = closed;
      for (const pending of this.pending.values()) pending.reject(closed);
      this.pending.clear();
      for (const listeners of this.events.values()) for (const listener of listeners) listener.reject(closed);
      this.events.clear();
    });
    await new Promise((resolve, reject) => {
      this.socket.addEventListener("open", () => resolve(), { once: true });
      this.socket.addEventListener("error", event => reject(new Error(
        `CDP WebSocket ${this.url} connection failed: ${inspect(event, {
          depth: null, maxArrayLength: null, maxStringLength: null,
        })}`,
      )), { once: true });
      this.socket.addEventListener("close", () => reject(this.closed), { once: true });
    });
  }
  send(method, params) {
    if (this.closed) return Promise.reject(new Error(`${method}: ${this.closed.message}`));
    const id = this.issued.push(method);
    return new Promise((resolve, reject) => {
      this.pending.set(id, { method, resolve, reject });
      try {
        this.socket.send(JSON.stringify({ id, method, params }));
      } catch (error) {
        this.pending.delete(id);
        reject(new Error(`CDP ${method} could not be sent: ${error.message}`));
      }
    });
  }
  // Resolves with the params of the next `method` event on this target.
  next(method) {
    if (this.closed) return Promise.reject(new Error(`${method}: ${this.closed.message}`));
    return new Promise((resolve, reject) => {
      if (!this.events.has(method)) this.events.set(method, []);
      this.events.get(method).push({ resolve, reject });
    });
  }
  close() {
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.close();
  }
}
