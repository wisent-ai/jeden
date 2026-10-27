import { readFile, writeFile, mkdir, realpath } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';

const FRAME = 'JEDEN_EXTENSION\t';
// The host is aborted when the session cancels it, which arrives as a
// signal; nothing here ends an extension because an interval elapsed.
const abortController = new AbortController();
process.once('SIGTERM', () => abortController.abort(new Error('extension operation cancelled')));
process.once('SIGINT', () => abortController.abort(new Error('extension operation cancelled')));
const allowedEvents = Object.freeze({
  SessionStart: true, UserPromptSubmit: true, PreToolUse: true, PostToolUse: true, Stop: true, TurnStart: true,
  TurnEnd: true, Message: true, Approval: true, Retry: true, ToolProgress: true,
});
// The grant each declared tool permission needs.
const permissionGrants = Object.freeze({ write: 'allowWrite', command: 'allowCommand' });
const loginMethods = Object.freeze({ device_code: true, paste: true, api_key: true });
// The exports a module may hold its extension under, in the order they are looked for.
const extensionExports = Object.freeze({ default: true, extension: true, activate: true, tool: true, tools: true });
// The lists a module's returned object may declare.
const declaredLists = Object.freeze({ tools: true, commands: true, hooks: true, capabilities: true, providers: true, models: true });
// A tool's execute that names only its input; one naming more takes the call id, input, update, context and signal.
const inputOnlyArity = ((input) => input).length;

function frame(value) { console.log(`${FRAME}${JSON.stringify(value)}`); }
function errorText(error) { return error instanceof Error ? error.message : String(error); }
/** A variable Jeden sets on every host it starts; unset, this is not that host. */
function required(name) {
  const value = process.env[name];
  if (value === undefined) throw new Error(`${name} is not set: the extension host runs only as Jeden starts it`);
  return value;
}
function json(name) {
  try { return JSON.parse(required(name)); } catch (error) { throw new Error(`${name} is not JSON: ${errorText(error)}`); }
}
function grant(name) {
  const value = json(name);
  if (typeof value !== 'boolean') throw new Error(`${name} must be true or false, not ${JSON.stringify(value)}`);
  return value;
}
function wholeNumber(name) {
  const value = json(name);
  if (!Number.isSafeInteger(value)) throw new Error(`${name} must be a whole number, not ${JSON.stringify(value)}`);
  return value;
}

let settings;
function readSettings() {
  const artifactDir = process.env.JEDEN_EXTENSION_ARTIFACT_DIR;
  const stated = {
    mode: required('JEDEN_EXTENSION_MODE'), cwd: resolve(required('JEDEN_EXTENSION_CWD')), artifactRoot: null,
    allowWrite: grant('JEDEN_EXTENSION_ALLOW_WRITE'), allowCommand: grant('JEDEN_EXTENSION_ALLOW_COMMAND'),
    generation: wholeNumber('JEDEN_EXTENSION_GENERATION'), abiVersion: wholeNumber('JEDEN_EXTENSION_ABI_VERSION'),
  };
  // Only an operation with an artifact directory lets extensions write artifacts.
  if (artifactDir !== undefined) stated.artifactRoot = resolve(artifactDir);
  return stated;
}

function validName(value) { return typeof value === 'string' && /^[\w.-]+$/.test(value); }
function text(value, field) {
  if (typeof value !== 'string') throw new Error(`${field} must be a string, not ${JSON.stringify(value)}`);
  return value;
}
function jailed(root, candidate) {
  const target = resolve(root, text(candidate, 'path'));
  const rel = relative(root, target);
  if (rel === '' || (rel !== '..' && !rel.startsWith(`..${sep}`) && !isAbsolute(rel))) return target;
  throw new Error(`path escapes root: ${candidate}`);
}

function hidden(node, values) {
  for (const [key, value] of Object.entries(values)) Object.defineProperty(node, key, { value, writable: true, enumerable: false });
  return node;
}
function schemaNode(type, extra) {
  const node = { type, ...extra };
  return hidden(node, {
    _optional: false, _hasDefault: false, _defaultValue: undefined,
    optional: () => { node._optional = true; return node; },
    default: (value) => { node._hasDefault = true; node._defaultValue = value; node._optional = true; return node; },
    describe: (description) => { node.description = description; return node; },
    array: () => schemaNode('array', { items: toJsonSchema(node) }),
  });
}
/** The JSON schema a zod-like or typebox-like node stands for; any other object is taken as a schema already. */
function toJsonSchema(value) {
  if (value === null || typeof value !== 'object') throw new Error(`a schema must be an object, not ${JSON.stringify(value)}`);
  if (typeof value._jedenSchema === 'function') return value._jedenSchema();
  const out = Object.fromEntries(Object.entries(value).filter(([key]) => !key.startsWith('_')));
  if (value._hasDefault) out.default = value._defaultValue;
  return out;
}
function zodObject(shape) {
  const node = schemaNode('object');
  return hidden(node, {
    _jedenSchema: () => {
      const properties = {}; const listed = [];
      for (const [key, value] of Object.entries(shape)) { properties[key] = toJsonSchema(value); if (value._optional !== true) listed.push(key); }
      const schema = { type: 'object', properties };
      if (listed.length) schema.required = listed;
      return schema;
    },
  });
}
const zod = { object: zodObject, string: () => schemaNode('string'), number: () => schemaNode('number'), boolean: () => schemaNode('boolean'), array: (item) => schemaNode('array', { items: toJsonSchema(item) }), enum: (values) => schemaNode('string', { enum: values }), any: () => ({}), unknown: () => ({}) };
const typebox = { Type: { Object: (properties) => ({ type: 'object', properties, required: Object.keys(properties).filter((key) => properties[key]._optional !== true) }), String: () => ({ type: 'string' }), Number: () => ({ type: 'number' }), Boolean: () => ({ type: 'boolean' }), Array: (items) => ({ type: 'array', items }), Optional: (schema) => ({ ...schema, _optional: true }) } };

/** The values of `raw` stated under any of `keys`: none or one, refused when stated under two. */
function statedOnce(raw, keys, what) {
  const stated = keys.filter((key) => raw[key] !== undefined);
  const [, ...again] = stated;
  if (again.length) throw new Error(`${what} is stated more than once (${stated.join(', ')})`);
  return stated.map((key) => raw[key]);
}
function normalizeTool(raw, source) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('tool must be an object');
  if (!validName(raw.name)) throw new Error(`invalid tool name: ${raw.name}`);
  if (typeof raw.description !== 'string' || !raw.description.trim()) throw new Error(`tool.description is required for ${raw.name}`);
  if (typeof raw.execute !== 'function') throw new Error(`tool.execute is required for ${raw.name}`);
  const tool = { kind: 'tool', name: raw.name, description: raw.description, input: { type: 'object', properties: {} }, permission: null, source, execute: raw.execute };
  if (raw.permission !== undefined && raw.permission !== null) {
    if (!Object.hasOwn(permissionGrants, raw.permission)) throw new Error(`invalid permission for ${raw.name}: ${raw.permission}`);
    tool.permission = raw.permission;
  }
  // A tool that states no input takes none.
  for (const schema of statedOnce(raw, ['parameters', 'input', 'schema'], `the input of ${raw.name}`)) tool.input = toJsonSchema(schema);
  return tool;
}
function normalizeCommand(raw, source) {
  if (!raw || typeof raw !== 'object' || !validName(raw.name)) throw new Error('command.name is invalid');
  const [prompt] = statedOnce(raw, ['prompt', 'template', 'body'], `the prompt of command ${raw.name}`);
  if (typeof prompt !== 'string' || !prompt.trim()) throw new Error(`command prompt is required for ${raw.name}`);
  const command = { kind: 'command', name: raw.name, prompt, source };
  if (raw.description !== undefined) command.description = text(raw.description, `command ${raw.name} description`);
  return command;
}
function normalizeHook(event, handler, source, matcher) {
  if (typeof event !== 'string' || allowedEvents[event] !== true) throw new Error(`unsupported extension event: ${event}`);
  if (typeof handler !== 'function') throw new Error(`hook handler is required for ${event}`);
  const hook = { kind: 'hook', event, source, handler };
  if (matcher !== undefined) hook.matcher = text(matcher, `${event} hook matcher`);
  return hook;
}
/** Runs `command` with `args` in the workspace (or `options.cwd` inside it) and answers its whole output. */
async function exec(command, args, options) {
  if (!settings.allowCommand) throw new Error('extension exec requires --allow-command');
  if (typeof command !== 'string' || !command) throw new Error('command is required');
  if (!Array.isArray(args) || args.some((arg) => typeof arg !== 'string')) throw new Error('args must be an array of strings');
  const childCwd = options?.cwd === undefined ? settings.cwd : jailed(settings.cwd, options.cwd);
  return await new Promise((done, reject) => {
    const child = spawn(command, args, { cwd: childCwd, env: { ...process.env, ...options?.env }, stdio: ['ignore', 'pipe', 'pipe'], signal: abortController.signal });
    const stdout = []; const stderr = [];
    child.stdout.on('data', (chunk) => stdout.push(chunk));
    child.stderr.on('data', (chunk) => stderr.push(chunk));
    child.on('error', reject);
    child.on('close', (code, terminationSignal) => done({ code, signal: terminationSignal, stdout: Buffer.concat(stdout).toString('utf8'), stderr: Buffer.concat(stderr).toString('utf8') }));
  });
}
async function artifact(name, content) {
  if (settings.artifactRoot === null) throw new Error('extension artifact bridge is unavailable: this operation keeps no artifacts');
  if (!settings.allowWrite) throw new Error('extension artifact requires --allow-write');
  if (!validName(name)) throw new Error('invalid artifact name');
  const path = jailed(settings.artifactRoot, name);
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, typeof content === 'string' ? content : JSON.stringify(content, null, '\t'), { flag: 'wx' });
  return path;
}

function eachListed(list, what, register) {
  if (list === undefined) return;
  if (!Array.isArray(list)) throw new Error(`${what} must be a list`);
  for (const raw of list) register(raw);
}
async function activate(file) {
  const importFile = await realpath(file);
  const tools = []; const commands = []; const hooks = []; const capabilities = []; const providers = []; const models = [];
  const { cwd, generation, abiVersion } = settings;
  const api = {
    abiVersion, cwd, generation, signal: abortController.signal, hasUI: false, ui: null, zod, typebox, pi: null,
    logger: { debug() {}, info() {}, warn() {}, error() {} },
    registerTool(raw) { tools.push(normalizeTool(raw, file)); },
    registerCommand(raw) { commands.push(normalizeCommand(raw, file)); },
    // on(event, handler) or on(event, toolMatcher, handler).
    on(event, first, second) {
      if (typeof first === 'function') hooks.push(normalizeHook(event, first, file));
      else hooks.push(normalizeHook(event, second, file, first));
    },
    registerCapability(raw) {
      if (!raw || typeof raw !== 'object' || !validName(raw.id)) throw new Error('capability.id is invalid');
      const capability = { id: raw.id, kind: text(raw.kind, `capability ${raw.id} kind`), version: text(raw.version, `capability ${raw.id} version`) };
      if (raw.description !== undefined) capability.description = text(raw.description, `capability ${raw.id} description`);
      capabilities.push(capability);
    },
    registerProvider(raw) {
      if (!raw || typeof raw !== 'object' || !validName(raw.id)) throw new Error('provider.id is invalid');
      if (typeof raw.displayName !== 'string' || !raw.displayName.trim()) throw new Error(`provider.displayName is required for ${raw.id}`);
      if (!Array.isArray(raw.loginMethods) || raw.loginMethods.some((method) => loginMethods[method] !== true)) throw new Error(`provider.loginMethods must list device_code, paste or api_key for ${raw.id}`);
      const provider = { id: raw.id, displayName: raw.displayName, loginMethods: raw.loginMethods, available: raw.available !== false };
      if (raw.unavailableReason !== undefined) provider.unavailableReason = text(raw.unavailableReason, `provider ${raw.id} unavailableReason`);
      providers.push(provider);
    },
    registerModel(raw) {
      if (!raw || typeof raw !== 'object' || !validName(raw.id)) throw new Error('model.id is invalid');
      for (const field of ['contextWindow', 'maxOutputTokens']) {
        const value = raw[field];
        if (value !== undefined && value !== null && !(Number.isSafeInteger(value) && Math.abs(value) === value)) throw new Error(`model.${field} must be a whole number of tokens for ${raw.id}`);
      }
      models.push({ ...raw, id: raw.id, available: raw.available !== false });
    },
    exec,
    readText(path) { return readFile(jailed(cwd, path), 'utf8'); },
    artifact,
  };
  api.pi = api;
  const imported = await import(`${pathToFileURL(importFile).href}?jeden_generation=${generation}&load=${randomUUID()}`);
  const exported = Object.keys(extensionExports).find((name) => imported[name] !== undefined);
  if (exported === undefined) throw new Error(`${file} exports none of ${Object.keys(extensionExports).join(', ')}`);
  const factory = imported[exported];
  const produced = typeof factory === 'function' ? await factory(api) : factory;
  if (produced && typeof produced === 'object' && !Array.isArray(produced) && Object.keys(declaredLists).some((key) => produced[key] !== undefined)) {
    eachListed(produced.tools, 'tools', api.registerTool);
    eachListed(produced.commands, 'commands', api.registerCommand);
    eachListed(produced.capabilities, 'capabilities', api.registerCapability);
    eachListed(produced.providers, 'providers', api.registerProvider);
    eachListed(produced.models, 'models', api.registerModel);
    eachListed(produced.hooks, 'hooks', (raw) => hooks.push(normalizeHook(raw.event, raw.handler, file, raw.matcher)));
  } else {
    for (const raw of (Array.isArray(produced) ? produced : [produced]).filter(Boolean)) api.registerTool(raw);
  }
  return { tools, commands, hooks, capabilities, providers, models };
}

async function discover() {
  const extensions = [];
  for (const file of json('JEDEN_EXTENSION_FILES')) {
    try {
      const loaded = await activate(file);
      extensions.push({ source: file, active: true, health: 'healthy', tools: loaded.tools.map(({ execute, ...rest }) => rest), commands: loaded.commands, hooks: loaded.hooks.map(({ handler, ...rest }) => rest), capabilities: loaded.capabilities, providers: loaded.providers, models: loaded.models });
    } catch (error) {
      extensions.push({ source: file, active: false, health: 'unhealthy', error: errorText(error), tools: [], commands: [], hooks: [], capabilities: [], providers: [], models: [] });
    }
  }
  return { ok: true, abiVersion: settings.abiVersion, extensions };
}
async function executeTool() {
  const source = required('JEDEN_EXTENSION_SOURCE');
  const target = required('JEDEN_EXTENSION_TARGET');
  const input = json('JEDEN_EXTENSION_INPUT');
  const { cwd, generation } = settings;
  const tool = (await activate(source)).tools.find((candidate) => candidate.name === target);
  if (!tool) throw new Error(`extension tool not found: ${target}`);
  if (tool.permission !== null && !settings[permissionGrants[tool.permission]]) throw new Error(`${target} requires --allow-${tool.permission}`);
  const updates = [];
  const update = (value) => { updates.push(value); console.log(`JEDEN_EXTENSION_PROGRESS\t${JSON.stringify(value)}`); };
  // A tool that installs new extension material has to be able to ask this
  // session to load it. The session process holds the registry, so the
  // request travels as a marker file the caller consumes right after this
  // tool answers; the promise resolves at once because the reload happens
  // outside this short-lived host.
  const reloadRequest = resolve(cwd, '.jeden/runtime/extensions/reload-request.json');
  const requestReload = async () => {
    await mkdir(dirname(reloadRequest), { recursive: true });
    await writeFile(reloadRequest, JSON.stringify({ tool: target, source, generation, at: new Date().toISOString() }));
    return { reloaded: true, deferred: true };
  };
  const context = { cwd, toolName: target, source, generation, signal: abortController.signal, artifact, exec, requestReload };
  const result = tool.execute.length > inputOnlyArity
    ? await tool.execute(`jeden-${generation}-${randomUUID()}`, input, update, context, abortController.signal)
    : await tool.execute(input);
  return { ok: true, result, updates };
}
async function fireHook() {
  const event = required('JEDEN_EXTENSION_EVENT');
  const payload = json('JEDEN_EXTENSION_INPUT');
  const hookIndex = wholeNumber('JEDEN_EXTENSION_HOOK_INDEX');
  const loaded = await activate(required('JEDEN_EXTENSION_SOURCE'));
  const hook = loaded.hooks.filter((candidate) => candidate.event === event)[hookIndex];
  if (!hook) throw new Error(`extension hook not found: ${event}#${hookIndex}`);
  const { cwd, generation } = settings;
  const answered = await hook.handler(payload, { cwd, event, generation, artifact, exec });
  // A hook that returned nothing answers null: Jeden reads every hook's result.
  if (answered === undefined) return { ok: true, result: null };
  return { ok: true, result: answered };
}

const modes = Object.freeze({ discover, execute_tool: executeTool, fire_hook: fireHook });
try {
  settings = readSettings();
  if (!Object.hasOwn(modes, settings.mode)) throw new Error(`unknown extension mode: ${settings.mode}`);
  frame(await modes[settings.mode]());
} catch (error) {
  // The frame carries the cause to Jeden; rethrown, it also ends the host unsuccessfully.
  frame({ ok: false, error: errorText(error) });
  throw error;
}
