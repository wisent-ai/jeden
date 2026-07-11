import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { dirname, relative, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const FRAME = 'JEDEN_EXTENSION\t';
const MAX_CAPTURE = 100_000;
const mode = process.env.JEDEN_EXTENSION_MODE || 'discover';
const cwd = resolve(process.env.JEDEN_EXTENSION_CWD || '.');
const artifactRoot = process.env.JEDEN_EXTENSION_ARTIFACT_DIR ? resolve(process.env.JEDEN_EXTENSION_ARTIFACT_DIR) : null;
const allowWrite = process.env.JEDEN_EXTENSION_ALLOW_WRITE === '1';
const abortController = new AbortController();
const timeoutMs = Math.max(1, Number(process.env.JEDEN_EXTENSION_TIMEOUT_MS || '60000'));
const abortTimer = setTimeout(() => abortController.abort(new Error('extension operation timed out')), timeoutMs);
abortTimer.unref();
process.once('SIGTERM', () => abortController.abort(new Error('extension operation cancelled')));
process.once('SIGINT', () => abortController.abort(new Error('extension operation cancelled')));
const allowedEvents = Object.freeze({
  SessionStart: true,
  UserPromptSubmit: true,
  PreToolUse: true,
  PostToolUse: true,
  Stop: true,
  TurnStart: true,
  TurnEnd: true,
  Message: true,
  Approval: true,
  Retry: true,
  ToolProgress: true,
});
const allowCommand = process.env.JEDEN_EXTENSION_ALLOW_COMMAND === '1';
const generation = Number(process.env.JEDEN_EXTENSION_GENERATION || '0');

function frame(value) { console.log(`${FRAME}${JSON.stringify(value)}`); }
function errorText(error) { return error instanceof Error ? error.message : String(error); }
function validName(value) { return typeof value === 'string' && /^[A-Za-z0-9._-]{1,80}$/.test(value); }
function validArtifactName(value) { return typeof value === 'string' && /^[A-Za-z0-9._-]{1,120}$/.test(value); }
function jailed(root, candidate) {
  const target = resolve(root, String(candidate || '.'));
  const rel = relative(root, target);
  if (rel === '' || (rel !== '..' && !rel.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) && !rel.startsWith('/'))) return target;
  throw new Error(`path escapes root: ${candidate}`);
}
function schemaNode(type, extra = {}) {
  const node = { type, ...extra, _optional: false, _hasDefault: false, _defaultValue: undefined };
  node.optional = () => { node._optional = true; return node; };
  node.default = (value) => { node._hasDefault = true; node._defaultValue = value; node._optional = true; return node; };
  node.describe = (description) => { node.description = description; return node; };
  node.array = () => schemaNode('array', { items: toJsonSchema(node) });
  return node;
}
function toJsonSchema(value) {
  if (!value || typeof value !== 'object') return {};
  if (typeof value._jedenSchema === 'function') return value._jedenSchema();
  if (typeof value.type === 'string') {
    const out = {};
    for (const [key, inner] of Object.entries(value)) if (!key.startsWith('_') && !['optional', 'default', 'describe', 'array'].includes(key)) out[key] = inner;
    if (value._hasDefault) out.default = value._defaultValue;
    return out;
  }
  return value;
}
function zodObject(shape = {}) {
  const node = schemaNode('object');
  node._jedenSchema = () => {
    const properties = {}; const required = [];
    for (const [key, value] of Object.entries(shape)) { properties[key] = toJsonSchema(value); if (!value || value._optional !== true) required.push(key); }
    return required.length ? { type: 'object', properties, required } : { type: 'object', properties };
  };
  return node;
}
const zod = { object: zodObject, string: () => schemaNode('string'), number: () => schemaNode('number'), boolean: () => schemaNode('boolean'), array: (item) => schemaNode('array', { items: toJsonSchema(item) }), enum: (values) => schemaNode('string', { enum: values }), any: () => ({}), unknown: () => ({}) };
const typebox = { Type: { Object: (properties = {}) => ({ type: 'object', properties, required: Object.keys(properties).filter((key) => !properties[key]?._optional) }), String: () => ({ type: 'string' }), Number: () => ({ type: 'number' }), Boolean: () => ({ type: 'boolean' }), Array: (items = {}) => ({ type: 'array', items }), Optional: (schema = {}) => ({ ...schema, _optional: true }) } };

function normalizeTool(raw, source) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('tool must be an object');
  if (!validName(raw.name)) throw new Error(`invalid tool name: ${raw.name}`);
  if (typeof raw.description !== 'string' || !raw.description.trim()) throw new Error(`tool.description is required for ${raw.name}`);
  if (typeof raw.execute !== 'function') throw new Error(`tool.execute is required for ${raw.name}`);
  const permission = raw.permission == null ? null : String(raw.permission);
  if (permission && !['write', 'command'].includes(permission)) throw new Error(`invalid permission for ${raw.name}: ${permission}`);
  return { kind: 'tool', name: raw.name, description: raw.description, input: toJsonSchema(raw.parameters || raw.input || raw.schema || {}), permission, source, execute: raw.execute };
}
function normalizeCommand(raw, source) {
  if (!raw || typeof raw !== 'object' || !validName(raw.name)) throw new Error('command.name is invalid');
  const prompt = raw.prompt ?? raw.template ?? raw.body;
  if (typeof prompt !== 'string' || !prompt.trim()) throw new Error(`command prompt is required for ${raw.name}`);
  return { kind: 'command', name: raw.name, description: typeof raw.description === 'string' ? raw.description : '', prompt, source };
}
function normalizeHook(event, handler, source, matcher = '') {
  if (typeof event !== 'string' || allowedEvents[event] !== true) throw new Error(`unsupported extension event: ${event}`);
  if (typeof handler !== 'function') throw new Error(`hook handler is required for ${event}`);
  return { kind: 'hook', event, matcher: String(matcher || ''), source, handler };
}
async function exec(command, args = [], options = {}) {
  if (!allowCommand) throw new Error('extension exec requires --allow-command');
  if (typeof command !== 'string' || !command) throw new Error('command is required');
  if (!Array.isArray(args)) throw new Error('args must be an array');
  for (const arg of args) {
    if (typeof arg !== 'string') throw new Error('each arg must be a string');
  }
  const childCwd = options.cwd ? jailed(cwd, options.cwd) : cwd;
  return await new Promise((done, reject) => {
    const child = spawn(command, args, { cwd: childCwd, env: { ...process.env, ...(options.env || {}) }, stdio: ['ignore', 'pipe', 'pipe'], signal: abortController.signal });
    let stdout = ''; let stderr = '';
    child.stdout.on('data', (chunk) => { stdout = (stdout + chunk.toString('utf8')).slice(0, MAX_CAPTURE); });
    child.stderr.on('data', (chunk) => { stderr = (stderr + chunk.toString('utf8')).slice(0, MAX_CAPTURE); });
    child.on('error', reject);
    child.on('close', (code, terminationSignal) => done({ code, signal: terminationSignal, stdout, stderr }));
  });
}
async function artifact(name, content) {
  if (!artifactRoot) throw new Error('extension artifact bridge is unavailable');
  if (!allowWrite) throw new Error('extension artifact requires --allow-write');
  if (!validArtifactName(name)) throw new Error('invalid artifact name');
  const path = jailed(artifactRoot, name);
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, typeof content === 'string' ? content : JSON.stringify(content, null, 2), { flag: 'wx' });
  return path;
}

async function activate(file) {
  const tools = []; const commands = []; const hooks = []; const capabilities = []; const providers = []; const models = [];
  const api = {
    abiVersion: 1, cwd, generation, signal: abortController.signal, hasUI: false, ui: null, zod, typebox, pi: null,
    logger: { debug() {}, info() {}, warn() {}, error() {} },
    registerTool(raw) { tools.push(normalizeTool(raw, file)); },
    registerCommand(raw) { commands.push(normalizeCommand(raw, file)); },
    on(event, matcherOrHandler, maybeHandler) {
      const handler = typeof matcherOrHandler === 'function' ? matcherOrHandler : maybeHandler;
      const matcher = typeof matcherOrHandler === 'string' ? matcherOrHandler : '';
      hooks.push(normalizeHook(event, handler, file, matcher));
    },
    registerCapability(raw) {
      if (!raw || typeof raw !== 'object' || !validName(raw.id)) throw new Error('capability.id is invalid');
      capabilities.push({ id: raw.id, kind: String(raw.kind || 'extension'), version: String(raw.version || '1'), description: String(raw.description || '') });
    },
    registerProvider(raw) {
      if (!raw || typeof raw !== 'object' || !validName(raw.id)) throw new Error('provider.id is invalid');
      if (typeof raw.displayName !== 'string' || !raw.displayName.trim()) throw new Error(`provider.displayName is required for ${raw.id}`);
      const loginMethods = raw.loginMethods || [];
      if (!Array.isArray(loginMethods) || loginMethods.some((method) => !['device_code', 'paste', 'api_key'].includes(method))) throw new Error(`provider.loginMethods is invalid for ${raw.id}`);
      providers.push({ id: raw.id, displayName: raw.displayName, loginMethods, available: raw.available !== false, unavailableReason: raw.unavailableReason ?? null });
    },
    registerModel(raw) {
      if (!raw || typeof raw !== 'object' || !validName(raw.id)) throw new Error('model.id is invalid');
      for (const field of ['contextWindow', 'maxOutputTokens']) if (raw[field] != null && (!Number.isSafeInteger(raw[field]) || raw[field] < 0)) throw new Error(`model.${field} is invalid for ${raw.id}`);
      models.push({ ...raw, id: raw.id, available: raw.available !== false });
    },
    exec,
    readText(path) { return readFile(jailed(cwd, path), 'utf8'); },
    artifact,
  };
  api.pi = api;
  const imported = await import(`${pathToFileURL(file).href}?jeden_generation=${generation}&nonce=${Date.now()}`);
  const factory = imported.default ?? imported.extension ?? imported.activate ?? imported.tool ?? imported.tools;
  let produced = typeof factory === 'function' ? await factory(api) : factory;
  if (produced && typeof produced === 'object' && !Array.isArray(produced) && (produced.tools || produced.commands || produced.hooks || produced.capabilities || produced.providers || produced.models)) {
    for (const raw of produced.tools || []) api.registerTool(raw);
    for (const raw of produced.commands || []) api.registerCommand(raw);
    for (const raw of produced.capabilities || []) api.registerCapability(raw);
    for (const raw of produced.providers || []) api.registerProvider(raw);
    for (const raw of produced.models || []) api.registerModel(raw);
    for (const raw of produced.hooks || []) api.on(raw.event, raw.matcher || '', raw.handler);
  } else {
    for (const raw of (Array.isArray(produced) ? produced : [produced]).filter(Boolean)) api.registerTool(raw);
  }
  return { tools, commands, hooks, capabilities, providers, models };
}

async function discover() {
  const files = JSON.parse(process.env.JEDEN_EXTENSION_FILES || '[]');
  const extensions = [];
  for (const file of files) {
    try {
      const loaded = await activate(file);
      extensions.push({ source: file, active: true, health: 'healthy', tools: loaded.tools.map(({ execute, ...rest }) => rest), commands: loaded.commands, hooks: loaded.hooks.map(({ handler, ...rest }) => rest), capabilities: loaded.capabilities, providers: loaded.providers, models: loaded.models });
    } catch (error) {
      extensions.push({ source: file, active: false, health: 'unhealthy', error: errorText(error), tools: [], commands: [], hooks: [], capabilities: [], providers: [], models: [] });
    }
  }
  return { ok: true, abiVersion: 1, extensions };
}
async function executeTool() {
  const source = process.env.JEDEN_EXTENSION_SOURCE;
  const target = process.env.JEDEN_EXTENSION_TARGET;
  const input = JSON.parse(process.env.JEDEN_EXTENSION_INPUT || '{}');
  const loaded = await activate(source);
  const tool = loaded.tools.find((candidate) => candidate.name === target);
  if (!tool) throw new Error(`extension tool not found: ${target}`);
  if (tool.permission === 'write' && !allowWrite) throw new Error(`${target} requires --allow-write`);
  if (tool.permission === 'command' && !allowCommand) throw new Error(`${target} requires --allow-command`);
  const updates = [];
  const update = (value) => { updates.push(value); console.log(`JEDEN_EXTENSION_PROGRESS\t${JSON.stringify(value)}`); };
  const context = { cwd, toolName: target, source, generation, signal: abortController.signal, artifact, exec };
  const result = tool.execute.length <= 1 ? await tool.execute(input) : await tool.execute(`jeden-${generation}-${Date.now()}`, input, update, context, undefined);
  return { ok: true, result, updates };
}
async function fireHook() {
  const source = process.env.JEDEN_EXTENSION_SOURCE;
  const event = process.env.JEDEN_EXTENSION_EVENT;
  const payload = JSON.parse(process.env.JEDEN_EXTENSION_INPUT || '{}');
  const hookIndex = Number(process.env.JEDEN_EXTENSION_HOOK_INDEX || '0');
  const loaded = await activate(source);
  const matching = loaded.hooks.filter((hook) => hook.event === event);
  const hook = matching[hookIndex];
  if (!hook) throw new Error(`extension hook not found: ${event}#${hookIndex}`);
  const result = await hook.handler(payload, { cwd, event, generation, artifact, exec });
  return { ok: true, result: result ?? null };
}

try {
  const value = mode === 'discover' ? await discover() : mode === 'execute_tool' ? await executeTool() : mode === 'fire_hook' ? await fireHook() : (() => { throw new Error(`unknown extension mode: ${mode}`); })();
  frame(value);
} catch (error) {
  frame({ ok: false, error: errorText(error) });
  process.exitCode = 1;
}
