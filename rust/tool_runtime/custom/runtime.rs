use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use crate::tool_runtime::ToolRuntime;

fn custom_tool_runtime_has_typescript(cwd: &Path) -> bool {
    [std::env::var_os("HOME").map(PathBuf::from).map(|home| home.join(".jeden/tools")), Some(cwd.join(".jeden/tools"))]
        .into_iter()
        .flatten()
        .any(|dir| {
            fs::read_dir(dir).ok().into_iter().flatten().filter_map(Result::ok).any(|entry| {
                entry.path().extension().and_then(|ext| ext.to_str()) == Some("ts")
            })
        })
}

fn node_supports_strip_types(node: &str) -> bool {
    Command::new(node)
        .arg("--experimental-strip-types")
        .arg("--eval")
        .arg("")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub(crate) fn custom_tool(runtime: &ToolRuntime<'_>, tool: &str, input: &Value) -> Result<Value, String> {
    let runner = r#"
import { readdir, readFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { homedir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const MAX_OUTPUT_BYTES = 100000;
function cap(v) { return v.length <= MAX_OUTPUT_BYTES ? v : v.slice(0, MAX_OUTPUT_BYTES); }
const enableTs = process.env.JEDEN_ENABLE_TS_CUSTOM_TOOLS === '1';
function isLoadableModule(name) { return name.endsWith('.js') || name.endsWith('.mjs') || (enableTs && name.endsWith('.ts')); }
function unique(values) { return [...new Set(values)]; }
function jailPath(cwd, inputPath) {
  const root = resolve(cwd);
  const target = resolve(root, String(inputPath || '.'));
  const rel = relative(root, target);
  if (rel === '' || (rel.slice(0, 2) !== '..' && rel.slice(0, 1) !== '/')) return target;
  throw new Error(`path escapes cwd: ${inputPath}`);
}
function optionalEnum(value, allowed, label) {
  if (value == null) return null;
  const text = String(value);
  if (!allowed.has(text)) throw new Error(`invalid ${label}: ${text}`);
  return text;
}
function isToolName(value) { return typeof value === 'string' && /^[A-Za-z0-9._-]{1,80}$/.test(value); }
function exec(command, args = [], options = {}) {
  if (!command || typeof command !== 'string') throw new Error('command is required');
  if (!Array.isArray(args) || args.some((arg) => typeof arg !== 'string')) throw new Error('args must be strings');
  const cwd = options.cwd ? resolve(String(options.cwd)) : process.cwd();
  return new Promise((resolvePromise) => {
    const child = spawn(command, args, { cwd, env: { ...process.env, ...(options.env || {}) } });
    let stdout = ''; let stderr = '';
    child.stdout.on('data', (chunk) => { stdout = cap(stdout + chunk.toString('utf8')); });
    child.stderr.on('data', (chunk) => { stderr = cap(stderr + chunk.toString('utf8')); });
    child.on('close', (code, signal) => { resolvePromise({ code, signal, timedOut: false, stdout, stderr }); });
  });
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
  if (value.type && typeof value.type === 'string') {
    const out = {};
    for (const [key, inner] of Object.entries(value)) {
      if (!key.startsWith('_') && !['optional', 'default', 'describe', 'array'].includes(key)) out[key] = inner;
    }
    if (value._hasDefault) out.default = value._defaultValue;
    return out;
  }
  return value;
}
function validateParams(schema, input, path = 'params') {
  if (!schema || typeof schema !== 'object') return input || {};
  if ((schema.type || 'object') === 'object') {
    const source = input && typeof input === 'object' && !Array.isArray(input) ? input : {};
    const out = { ...source };
    const required = new Set(Array.isArray(schema.required) ? schema.required : []);
    for (const [key, prop] of Object.entries(schema.properties || {})) {
      if (out[key] === undefined) {
        if (prop && Object.prototype.hasOwnProperty.call(prop, 'default')) out[key] = prop.default;
        else if (required.has(key)) throw new Error(`${path}.${key} is required`);
      }
      if (out[key] !== undefined && prop && prop.type === 'object') out[key] = validateParams(prop, out[key], `${path}.${key}`);
    }
    return out;
  }
  return input;
}
function zodObject(shape = {}) {
  const obj = schemaNode('object');
  obj._jedenSchema = () => {
    const properties = {};
    const required = [];
    for (const [key, inner] of Object.entries(shape || {})) {
      properties[key] = toJsonSchema(inner);
      if (!inner || inner._optional !== true) required.push(key);
    }
    const out = { type: 'object', properties };
    if (required.length) out.required = required;
    return out;
  };
  return obj;
}
const zod = {
  object: zodObject,
  string: () => schemaNode('string'),
  number: () => schemaNode('number'),
  boolean: () => schemaNode('boolean'),
  array: (item) => schemaNode('array', { items: toJsonSchema(item) }),
  enum: (values) => schemaNode('string', { enum: values }),
  any: () => ({}),
  unknown: () => ({}),
};
const typebox = { Type: {
  Object: (properties) => ({ type: 'object', properties: properties || {}, required: Object.keys(properties || {}) }),
  String: () => ({ type: 'string' }),
  Number: () => ({ type: 'number' }),
  Boolean: () => ({ type: 'boolean' }),
  Array: (items) => ({ type: 'array', items: items || {} }),
  Optional: (schema) => ({ ...(schema || {}), _optional: true }),
} };

function normalizeTool(raw, source) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('tool must be an object');
  if (!raw.name || typeof raw.name !== 'string') throw new Error('tool.name is required');
  if (!isToolName(raw.name)) throw new Error(`invalid tool name: ${raw.name}`);
  if (!raw.description || typeof raw.description !== 'string') throw new Error(`tool.description is required for ${raw.name}`);
  if (typeof raw.execute !== 'function') throw new Error(`tool.execute is required for ${raw.name}`);
  return {
    name: raw.name,
    description: raw.description,
    input: toJsonSchema(raw.parameters || raw.input || raw.schema || {}),
    permission: optionalEnum(raw.permission, new Set(['write', 'command']), 'permission'),
    hook: optionalEnum(raw.hook, new Set(['read', 'edit', 'bash']), 'hook'),
    postHook: optionalEnum(raw.postHook, new Set(['read', 'edit', 'bash']), 'postHook'),
    source,
    execute: raw.execute,
  };
}
async function listToolFiles(dir) {
  try {
    const entries = await readdir(dir, { withFileTypes: true });
    return entries.filter((entry) => entry.isFile() && isLoadableModule(entry.name)).map((entry) => join(dir, entry.name)).sort();
  } catch (error) {
    if (error && (error.code === 'ENOENT' || error.code === 'ENOTDIR')) return [];
    throw error;
  }
}
async function loadToolFile(file, api) {
  const module = await import(`${pathToFileURL(file).href}?mtime=${Date.now()}`);
  const factory = module.default || module.tool || module.tools;
  const produced = typeof factory === 'function' ? await factory(api) : factory;
  const list = Array.isArray(produced) ? produced : [produced];
  return list.map((candidate) => normalizeTool(candidate, file));
}
async function main() {
  const cwd = process.env.JEDEN_CUSTOM_CWD;
  const target = process.env.JEDEN_CUSTOM_TOOL;
  const input = JSON.parse(process.env.JEDEN_CUSTOM_INPUT || '{}');
  const allowWrite = process.env.JEDEN_CUSTOM_ALLOW_WRITE === '1';
  const allowCommand = process.env.JEDEN_CUSTOM_ALLOW_COMMAND === '1';
  const api = {
    cwd: resolve(cwd),
    hasUI: false,
    ui: null,
    logger: { debug() {}, info() {}, warn() {}, error() {} },
    zod,
    typebox,
    pi: null,
    pushPendingAction: () => {},
    exec: (command, args, options = {}) => {
      if (!allowCommand) throw new Error('custom tool exec requires --allow-command');
      return exec(command, args, { cwd: resolve(cwd), ...options });
    },
    readText: (path) => readFile(jailPath(cwd, path), 'utf8'),
    dirname,
  };
  api.pi = api;
  const files = (await Promise.all(unique([join(homedir(), '.jeden', 'tools'), join(resolve(cwd), '.jeden', 'tools')]).map(listToolFiles))).flat();
  const errors = [];
  const seen = new Set();
  for (const file of files) {
    try {
      for (const loaded of await loadToolFile(file, api)) {
        if (seen.has(loaded.name)) throw new Error(`tool name conflict: ${loaded.name}`);
        seen.add(loaded.name);
        if (loaded.name !== target) continue;
        if (loaded.permission === 'write' && !allowWrite) throw new Error(`${target} requires --allow-write`);
        if (loaded.permission === 'command' && !allowCommand) throw new Error(`${target} requires --allow-command`);
        const params = validateParams(loaded.input, input || {});
        const result = loaded.execute.length <= 1
          ? await loaded.execute(params)
          : await loaded.execute(`jeden-${Date.now()}`, params, () => {}, { cwd: resolve(cwd), toolName: loaded.name, source: loaded.source }, undefined);
        return { ok: true, found: true, result };
      }
    } catch (error) {
      errors.push({ path: file, error: error instanceof Error ? error.message : String(error) });
    }
  }
  return { ok: false, found: false, errors };
}
main().then((value) => {
  console.log(`JEDEN_CUSTOM_RESULT\t${JSON.stringify(value)}`);
}).catch((error) => {
  console.log(`JEDEN_CUSTOM_RESULT\t${JSON.stringify({ ok: false, found: false, fatal: error instanceof Error ? error.message : String(error) })}`);
});
"#;
    let node = std::env::var("JEDEN_NODE").unwrap_or_else(|_| "node".into());
    let enable_ts = custom_tool_runtime_has_typescript(runtime.cwd) && node_supports_strip_types(&node);
    let mut command = Command::new(node);
    command.arg("--input-type=module");
    if enable_ts {
        command.arg("--experimental-strip-types");
    }
    let mut child = command
        .arg("-e")
        .arg(runner)
        .env("JEDEN_CUSTOM_CWD", runtime.cwd)
        .env("JEDEN_CUSTOM_TOOL", tool)
        .env("JEDEN_CUSTOM_INPUT", input.to_string())
        .env("JEDEN_CUSTOM_ALLOW_WRITE", if runtime.allow_write { "1" } else { "0" })
        .env("JEDEN_CUSTOM_ALLOW_COMMAND", if runtime.allow_command { "1" } else { "0" })
        .env("JEDEN_ENABLE_TS_CUSTOM_TOOLS", if enable_ts { "1" } else { "0" })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("custom tool bridge failed to start node: {e}"))?;
    let mut stdout_pipe = child.stdout.take().ok_or("custom tool bridge missing stdout")?;
    let mut stderr_pipe = child.stderr.take().ok_or("custom tool bridge missing stderr")?;
    let stdout_handle = std::thread::spawn(move || {
        let mut stdout = String::new();
        let _ = stdout_pipe.read_to_string(&mut stdout);
        stdout
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut stderr = String::new();
        let _ = stderr_pipe.read_to_string(&mut stderr);
        stderr
    });
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            let stdout = stdout_handle.join().unwrap_or_default();
            let stderr = stderr_handle.join().unwrap_or_default();
            if !status.success() { return Err(format!("custom tool bridge exited with {status}: {stderr}")); }
            let result_line = stdout.lines().rev().find_map(|line| line.strip_prefix("JEDEN_CUSTOM_RESULT\t"));
            let Some(result_line) = result_line else { return Err(format!("custom tool bridge returned no result: {stdout}{stderr}")); };
            let value: Value = serde_json::from_str(result_line).map_err(|e| e.to_string())?;
            if value.get("found").and_then(Value::as_bool) == Some(true) {
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            let errors = value.get("errors").cloned().unwrap_or(Value::Null);
            return Err(format!("Rust tool runtime has not ported tool: {tool}; custom tool not found; errors: {errors}"));
        }
        sleep(Duration::from_millis(25));
    }
}
