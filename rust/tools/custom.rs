use super::ToolInfo;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn custom_tool_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".jeden/tools"));
    }
    dirs.push(cwd.join(".jeden/tools"));
    dirs
}

fn has_typescript_custom_tool(cwd: &Path) -> bool {
    custom_tool_dirs(cwd).into_iter().any(|dir| {
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

fn load_custom_tools_via_factory(cwd: &Path) -> Option<Vec<ToolInfo>> {
    let runner = r#"
import { readdir } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
function schemaNode(type, extra = {}) {
  const node = { type, ...extra, _optional: false, _hasDefault: false, _defaultValue: undefined };
  node.optional = () => { node._optional = true; return node; };
  node.default = (value) => { node._hasDefault = true; node._defaultValue = value; node._optional = true; return node; };
  node.describe = (description) => { node.description = description; return node; };
  return node;
}
function toJsonSchema(value) {
  if (!value || typeof value !== 'object') return {};
  if (typeof value._jedenSchema === 'function') return value._jedenSchema();
  if (value.type && typeof value.type === 'string') {
    const out = {};
    for (const [key, inner] of Object.entries(value)) {
      if (!key.startsWith('_') && !['optional', 'default', 'describe'].includes(key)) out[key] = inner;
    }
    if (value._hasDefault) out.default = value._defaultValue;
    return out;
  }
  return value;
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
const enableTs = process.env.JEDEN_ENABLE_TS_CUSTOM_TOOLS === '1';
async function listFiles(dir) {
  try {
    const entries = await readdir(dir, { withFileTypes: true });
    return entries.filter((entry) => entry.isFile() && (entry.name.endsWith('.js') || entry.name.endsWith('.mjs') || (enableTs && entry.name.endsWith('.ts')))).map((entry) => join(dir, entry.name)).sort();
  } catch (error) {
    if (error && (error.code === 'ENOENT' || error.code === 'ENOTDIR')) return [];
    throw error;
  }
}
async function loadFile(file, api) {
  const module = await import(`${pathToFileURL(file).href}?mtime=${Date.now()}`);
  const factory = module.default || module.tool || module.tools;
  const produced = typeof factory === 'function' ? await factory(api) : factory;
  return (Array.isArray(produced) ? produced : [produced]).filter(Boolean).map((raw) => ({
    name: raw.name,
    description: raw.description,
    input: toJsonSchema(raw.parameters || raw.input || raw.schema || {}),
  }));
}
const cwd = resolve(process.env.JEDEN_CUSTOM_DISCOVERY_CWD || '.');
const api = { cwd, hasUI: false, ui: null, logger: { debug() {}, info() {}, warn() {}, error() {} }, zod, typebox, pi: null, pushPendingAction: () => {} };
api.pi = api;
const files = (await Promise.all([join(homedir(), '.jeden', 'tools'), join(cwd, '.jeden', 'tools')].map(listFiles))).flat();
const seen = new Set();
const out = [];
for (const file of files) {
  for (const tool of await loadFile(file, api)) {
    if (!tool.name || !tool.description || seen.has(tool.name)) continue;
    seen.add(tool.name);
    out.push(tool);
  }
}
console.log(`JEDEN_CUSTOM_TOOLS\t${JSON.stringify(out)}`);
"#;
    let node = env::var("JEDEN_NODE").unwrap_or_else(|_| "node".into());
    let enable_ts = has_typescript_custom_tool(cwd) && node_supports_strip_types(&node);
    let mut command = Command::new(node);
    command.arg("--input-type=module");
    if enable_ts {
        command.arg("--experimental-strip-types");
    }
    let mut child = command
        .arg("-e")
        .arg(runner)
        .env("JEDEN_CUSTOM_DISCOVERY_CWD", cwd)
        .env("JEDEN_ENABLE_TS_CUSTOM_TOOLS", if enable_ts { "1" } else { "0" })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout_pipe = child.stdout.take()?;
    let stdout_handle = std::thread::spawn(move || {
        let mut stdout = String::new();
        let _ = stdout_pipe.read_to_string(&mut stdout);
        stdout
    });
    let start = Instant::now();
    let stdout = loop {
        if let Some(status) = child.try_wait().ok()? {
            let stdout = stdout_handle.join().unwrap_or_default();
            if !status.success() { return None; }
            break stdout;
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_handle.join();
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let line = stdout.lines().rev().find_map(|line| line.strip_prefix("JEDEN_CUSTOM_TOOLS\t"))?;
    let values: Vec<Value> = serde_json::from_str(line).ok()?;
    Some(values.into_iter().filter_map(|value| {
        Some(ToolInfo {
            name: value.get("name")?.as_str()?.to_string(),
            description: value.get("description")?.as_str()?.to_string(),
            input: value.get("input").cloned().unwrap_or_else(|| json!({})),
        })
    }).collect())
}

pub(super) fn static_custom_tools(cwd: &Path, seen: &mut BTreeSet<String>) -> Vec<ToolInfo> {
    let mut tools = Vec::new();
    if let Some(loaded) = load_custom_tools_via_factory(cwd) {
        for tool in loaded {
            if seen.contains(&tool.name) { continue; }
            seen.insert(tool.name.clone());
            tools.push(tool);
        }
        return tools;
    }
    tools
}
