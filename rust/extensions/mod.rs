mod declarative;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, LazyLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

use crate::tools::ToolInfo;
use crate::capability::{CapabilityDescriptor as RegistryDescriptor, CapabilityHealth, CapabilityKind, CapabilityPolicy, FunctionTarget};

const ABI_VERSION: u32 = 1;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const INVOCATION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_EXTENSION_FILES: usize = 256;
const MAX_DESCRIPTOR_BYTES: usize = 2 * 1024 * 1024;
const HOST: &str = include_str!("host.mjs");

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolDescriptor {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub permission: Option<String>,
    pub source: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CommandDescriptor {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub prompt: String,
    pub source: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct HookDescriptor {
    pub event: String,
    #[serde(default)]
    pub matcher: String,
    pub source: PathBuf,
    #[serde(default)]
    pub index: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CapabilityDescriptor {
    pub id: String,
    pub kind: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize)]
struct HostExtension {
    source: PathBuf,
    active: bool,
    health: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    tools: Vec<ToolDescriptor>,
    #[serde(default)]
    commands: Vec<CommandDescriptor>,
    #[serde(default)]
    hooks: Vec<HookDescriptor>,
    #[serde(default)]
    capabilities: Vec<CapabilityDescriptor>,
    #[serde(default)]
    providers: Vec<crate::control_plane::weles::Provider>,
    #[serde(default)]
    models: Vec<crate::control_plane::brama::ModelEntry>,
    #[serde(skip)]
    precedence: usize,
}

#[derive(Clone, Debug)]
struct DeclarativeCapability {
    kind: &'static str,
    path: PathBuf,
    healthy: bool,
    error: Option<String>,
    precedence: usize,
}

#[derive(Clone, Debug)]
struct InstalledPluginRoot {
    id: String,
    version: String,
    path: PathBuf,
    enabled: bool,
}

#[derive(Clone, Debug)]
struct SourceSet {
    modules: Vec<PathBuf>,
    declarative: Vec<DeclarativeCapability>,
    installed_plugins: Vec<InstalledPluginRoot>,
    module_precedence: BTreeMap<PathBuf, usize>,
    fingerprint: u64,
}

#[derive(Clone, Debug)]
struct Registry {
    generation: u64,
    fingerprint: u64,
    extensions: Vec<HostExtension>,
    tools: BTreeMap<String, ToolDescriptor>,
    hooks: Vec<HookDescriptor>,
    command_dir: Option<PathBuf>,
    declarative: Vec<DeclarativeCapability>,
    declarative_runtime: declarative::Loaded,
    agent_dir: Option<PathBuf>,
    installed_plugins: Vec<InstalledPluginRoot>,
}

#[derive(Clone, Debug)]
pub(crate) struct ReloadReport {
    pub generation: u64,
    pub active_extensions: usize,
    pub unhealthy_extensions: usize,
    pub tools: usize,
    pub commands: usize,
    pub hooks: usize,
    pub capabilities: usize,
}

static REGISTRIES: LazyLock<RwLock<BTreeMap<PathBuf, Arc<Registry>>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

fn canonical_key(cwd: &Path) -> PathBuf {
    fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf())
}

fn module_file(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("js" | "mjs" | "ts")
        )
}

fn package_entries(root: &Path) -> Vec<PathBuf> {
    let manifest = fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    let entries = manifest
        .pointer("/jeden/extensions")
        .and_then(Value::as_array)
        .or_else(|| manifest.pointer("/pi/extensions").and_then(Value::as_array));
    entries
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|path| root.join(path))
        .filter(|path| module_file(path))
        .collect()
}

fn scan_modules(root: &Path, recursive_children: bool) -> Vec<PathBuf> {
    if module_file(root) {
        return vec![root.to_path_buf()];
    }
    if !root.is_dir() {
        return Vec::new();
    }
    let manifest = package_entries(root);
    if !manifest.is_empty() {
        return manifest;
    }
    for name in ["index.ts", "index.js", "index.mjs"] {
        let path = root.join(name);
        if module_file(&path) {
            return vec![path];
        }
    }
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if module_file(&path) {
                files.push(path);
            } else if recursive_children && path.is_dir() {
                files.extend(scan_modules(&path, false));
            }
            if files.len() >= MAX_EXTENSION_FILES {
                break;
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn declarative_paths(root: &Path, precedence: usize) -> Vec<DeclarativeCapability> {
    let mut values = Vec::new();
    for kind in ["commands", "hooks", "skills", "agents", "rules"] {
        let path = if kind == "hooks" {
            root.join("hooks.json")
        } else {
            root.join(kind)
        };
        if path.exists() {
            let health = if kind == "hooks" {
                fs::read_to_string(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|error| error.to_string()))
                    .map(|value| value.is_object())
                    .and_then(|valid| if valid { Ok(()) } else { Err("hooks manifest must be an object".into()) })
            } else {
                fs::read_dir(&path).map(|_| ()).map_err(|error| error.to_string())
            };
            values.push(DeclarativeCapability {
                kind,
                path,
                healthy: health.is_ok(),
                error: health.err(),
                precedence,
            });
        }
    }
    values
}
fn hash_path_tree(path: &Path, hasher: &mut DefaultHasher, remaining: &mut usize) {
    if *remaining == 0 {
        return;
    }
    *remaining -= 1;
    path.hash(hasher);
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    metadata.len().hash(hasher);
    metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .hash(hasher);
    if metadata.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        let mut children = entries.flatten().map(|entry| entry.path()).collect::<Vec<_>>();
        children.sort();
        for child in children {
            hash_path_tree(&child, hasher, remaining);
            if *remaining == 0 {
                break;
            }
        }
    }
}


fn read_json(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

fn config_value(cwd: &Path, key: &str) -> Value {
    let project = read_json(&cwd.join(".jeden/config.json"));
    if let Some(value) = project.get(key) {
        return value.clone();
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| read_json(&home.join(".jeden/config.json")))
        .and_then(|config| config.get(key).cloned())
        .unwrap_or(Value::Null)
}

fn installed_plugin_roots(cwd: &Path) -> Vec<InstalledPluginRoot> {
    let home = env::var_os("JEDEN_PLUGINS_HOME")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf());
    let mut roots = Vec::new();
    for scope in [cwd.to_path_buf(), home] {
        let registry = read_json(&scope.join(".jeden/plugins.json"));
        let Some(installed) = registry.get("installed").and_then(Value::as_object) else {
            continue;
        };
        for (registry_id, value) in installed {
            let Some(path) = value.get("path").and_then(Value::as_str) else {
                continue;
            };
            roots.push(InstalledPluginRoot {
                id: value.get("id").and_then(Value::as_str).unwrap_or(registry_id).to_string(),
                version: value.get("version").and_then(Value::as_str).unwrap_or("unversioned").to_string(),
                path: PathBuf::from(path),
                enabled: value.get("enabled").and_then(Value::as_bool) != Some(false),
            });
        }
    }
    roots.sort_by(|left, right| left.id.cmp(&right.id).then_with(|| left.path.cmp(&right.path)));
    roots
}

fn source_set(cwd: &Path) -> Result<SourceSet, String> {
    let mut modules = Vec::new();
    let mut declarative = Vec::new();
    let mut roots = vec![cwd.join(".jeden/extensions")];
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".jeden/extensions"));
        declarative.extend(declarative_paths(&home.join(".jeden"), 20_000));
        roots.push(home.join(".jeden/tools"));
    }
    declarative.extend(declarative_paths(&cwd.join(".jeden"), 30_000));
    roots.push(cwd.join(".jeden/tools"));
    let configured_extensions = config_value(cwd, "extensions");
    if let Some(configured) = configured_extensions.as_array() {
        for value in configured {
            if let Some(path) = value.as_str() {
                let path = PathBuf::from(path);
                roots.push(if path.is_absolute() { path } else { cwd.join(path) });
            }
        }
    }
    let installed_plugins = installed_plugin_roots(cwd);
    for (index, plugin) in installed_plugins.iter().enumerate() {
        if !plugin.enabled {
            continue;
        }
        roots.push(plugin.path.join("extensions"));
        roots.push(plugin.path.join("tools"));
        modules.extend(package_entries(&plugin.path));
        modules.extend(scan_modules(&plugin.path, false));
        declarative.extend(declarative_paths(&plugin.path, 10_000 + index));
    }
    for root in roots {
        modules.extend(scan_modules(&root, true));
    }
    modules.sort();
    modules.dedup();
    if modules.len() > MAX_EXTENSION_FILES {
        return Err(format!(
            "extension discovery exceeds the limit of {MAX_EXTENSION_FILES} modules"
        ));
    }
    let disabled_extensions = config_value(cwd, "disabledExtensions");
    let disabled: BTreeSet<String> = disabled_extensions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    modules.retain(|path| {
        let text = path.to_string_lossy();
        !disabled.iter().any(|id| text.ends_with(id) || text.contains(&format!("/{id}/")))
    });
    let home_jeden = env::var_os("HOME").map(PathBuf::from).map(|home| home.join(".jeden"));
    let mut module_precedence = BTreeMap::new();
    for module in &modules {
        let precedence = installed_plugins
            .iter()
            .enumerate()
            .find(|(_, plugin)| plugin.enabled && module.starts_with(&plugin.path))
            .map(|(index, _)| 10_000 + index)
            .or_else(|| home_jeden.as_ref().filter(|root| module.starts_with(root)).map(|_| 20_000))
            .unwrap_or(30_000);
        module_precedence.insert(module.clone(), precedence);
    }
    let mut hasher = DefaultHasher::new();
    for path in &modules {
        path.hash(&mut hasher);
        if let Ok(metadata) = fs::metadata(path) {
            metadata.len().hash(&mut hasher);
            metadata
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .hash(&mut hasher);
        }
    }
    for plugin in &installed_plugins {
        plugin.id.hash(&mut hasher);
        plugin.version.hash(&mut hasher);
        plugin.enabled.hash(&mut hasher);
    }
    let mut declarative_budget = 2_048usize;
    for item in &declarative {
        item.kind.hash(&mut hasher);
        item.path.hash(&mut hasher);
        item.precedence.hash(&mut hasher);
        hash_path_tree(&item.path, &mut hasher, &mut declarative_budget);
    }
    Ok(SourceSet {
        modules,
        declarative,
        installed_plugins,
        module_precedence,
        fingerprint: hasher.finish(),
    })
}

fn node_supports_typescript(node: &str) -> bool {
    Command::new(node)
        .args(["--experimental-strip-types", "--eval", ""])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn run_host(
    cwd: &Path,
    mode: &str,
    generation: u64,
    timeout: Duration,
    envs: &[(&str, String)],
    allow_write: bool,
    allow_command: bool,
    artifact_dir: Option<&Path>,
    operation: Option<&crate::tool_runtime::runtime_ops::OperationContext<'_>>,
) -> Result<Value, String> {
    if operation.is_some_and(|context| context.cancellation().is_cancelled()) {
        return Err("extension operation cancelled".into());
    }
    let timeout = operation
        .and_then(|context| context.deadline().and_then(|deadline| deadline.checked_duration_since(Instant::now())))
        .map(|remaining| remaining.min(timeout))
        .unwrap_or(timeout);
    if timeout.is_zero() {
        return Err("extension operation deadline exceeded".into());
    }
    let node = env::var("JEDEN_NODE").unwrap_or_else(|_| "node".into());
    let enable_ts = envs.iter().any(|(_, value)| value.contains(".ts")) && node_supports_typescript(&node);
    let mut command = Command::new(node);
    command.arg("--input-type=module");
    if enable_ts {
        command.arg("--experimental-strip-types");
    }
    command
        .args(["-e", HOST])
        .env("JEDEN_EXTENSION_MODE", mode)
        .env("JEDEN_EXTENSION_CWD", cwd)
        .env("JEDEN_EXTENSION_GENERATION", generation.to_string())
        .env(
            "JEDEN_EXTENSION_TIMEOUT_MS",
            timeout.as_millis().saturating_sub(250).max(1).to_string(),
        )
        .env("JEDEN_EXTENSION_ALLOW_WRITE", if allow_write { "1" } else { "0" })
        .env("JEDEN_EXTENSION_ALLOW_COMMAND", if allow_command { "1" } else { "0" })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = operation.map(|context| context.artifacts().root()).or(artifact_dir) {
        command.env("JEDEN_EXTENSION_ARTIFACT_DIR", dir);
    }
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("extension host failed to start: {error}"))?;
    let mut stdout = child.stdout.take().ok_or("extension host missing stdout")?;
    let mut stderr = child.stderr.take().ok_or("extension host missing stderr")?;
    let (progress_tx, progress_rx) = mpsc::channel();
    let stdout_progress = progress_tx.clone();
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut total = 0u64;
        loop {
            let count = stdout.read(&mut buffer).unwrap_or_default();
            if count == 0 { break; }
            total = total.saturating_add(count as u64);
            let remaining = (MAX_DESCRIPTOR_BYTES + 1).saturating_sub(bytes.len());
            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
            let _ = stdout_progress.send(("extension-stdout", count as u64, total));
        }
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let count = stderr.read(&mut buffer).unwrap_or_default();
            if count == 0 { break; }
            let remaining = (MAX_DESCRIPTOR_BYTES + 1).saturating_sub(bytes.len());
            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        bytes
    });
    drop(progress_tx);
    let started = Instant::now();
    let status = loop {
        while let Ok((stream, bytes, total_bytes)) = progress_rx.try_recv() {
            if let Some(context) = operation {
                context.progress(crate::tool_runtime::runtime_ops::OperationProgress { stream, bytes, total_bytes });
            }
        }
        if operation.is_some_and(|context| context.cancellation().is_cancelled()) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("extension operation cancelled".into());
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(format!("extension host timed out after {} seconds", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if let Some(context) = operation {
        let mut output = crate::tool_runtime::runtime_ops::BoundedOutput::new(
            "extension-host",
            context.output_limits(),
            context.artifacts().clone(),
        );
        output.write_chunk(&stdout).map_err(|error| error.to_string())?;
        output.write_chunk(&stderr).map_err(|error| error.to_string())?;
        output.finish().map_err(|error| error.to_string())?;
    }
    if stdout.len() > MAX_DESCRIPTOR_BYTES || stderr.len() > MAX_DESCRIPTOR_BYTES {
        return Err("extension host output exceeded 2 MiB".into());
    }
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    let line = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("JEDEN_EXTENSION\t"))
        .ok_or_else(|| format!("extension host returned no protocol frame: {stderr}"))?;
    let value: Value = serde_json::from_str(line)
        .map_err(|error| format!("invalid extension host frame: {error}"))?;
    if !status.success() || value.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_else(|| stderr.trim())
            .to_string());
    }
    Ok(value)
}

fn materialize_commands(cwd: &Path, generation: u64, commands: &[CommandDescriptor]) -> Result<Option<PathBuf>, String> {
    if commands.is_empty() {
        return Ok(None);
    }
    let root = cwd.join(".jeden/runtime/extensions");
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let stage = root.join(format!("commands-{generation}.stage-{}", std::process::id()));
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&stage).map_err(|error| error.to_string())?;
    for command in commands {
        let file = stage.join(format!("{}.md", command.name));
        let contents = if command.description.trim().is_empty() {
            command.prompt.clone()
        } else {
            format!(
                "---\ndescription: {}\n---\n{}",
                serde_json::to_string(&command.description.replace('\n', " "))
                    .unwrap_or_else(|_| "\"extension command\"".into()),
                command.prompt
            )
        };
        fs::write(file, contents).map_err(|error| error.to_string())?;
    }
    let final_dir = root.join(format!("commands-{generation}"));
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir).map_err(|error| error.to_string())?;
    }
    fs::rename(&stage, &final_dir).map_err(|error| error.to_string())?;
    Ok(Some(final_dir))
}
fn materialize_agents(
    cwd: &Path,
    generation: u64,
    agents: &BTreeMap<String, declarative::Agent>,
) -> Result<Option<PathBuf>, String> {
    if agents.is_empty() {
        return Ok(None);
    }
    let root = cwd.join(".jeden/runtime/extensions");
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let stage = root.join(format!("agents-{generation}.stage-{}", std::process::id()));
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&stage).map_err(|error| error.to_string())?;
    for agent in agents.values() {
        let file = stage.join(format!("{}.json", agent.id));
        let bytes = serde_json::to_vec_pretty(&agent.value).map_err(|error| error.to_string())?;
        fs::write(file, bytes).map_err(|error| error.to_string())?;
    }
    let final_dir = root.join(format!("agents-{generation}"));
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir).map_err(|error| error.to_string())?;
    }
    fs::rename(&stage, &final_dir).map_err(|error| error.to_string())?;
    Ok(Some(final_dir))
}


fn build_registry(cwd: &Path, sources: SourceSet, generation: u64) -> Result<Registry, String> {
    let files = serde_json::to_string(&sources.modules).map_err(|error| error.to_string())?;
    let response = run_host(
        cwd,
        "discover",
        generation,
        DISCOVERY_TIMEOUT,
        &[("JEDEN_EXTENSION_FILES", files)],
        false,
        false,
        None,
        None,
    )?;
    if response.get("abiVersion").and_then(Value::as_u64) != Some(ABI_VERSION as u64) {
        return Err("extension host ABI mismatch".into());
    }
    let mut extensions: Vec<HostExtension> = serde_json::from_value(
        response.get("extensions").cloned().unwrap_or_else(|| json!([])),
    )
    .map_err(|error| format!("invalid extension descriptors: {error}"))?;
    let source_set: BTreeSet<PathBuf> = sources.modules.iter().cloned().collect();
    extensions.retain(|extension| source_set.contains(&extension.source));
    for extension in &mut extensions {
        extension.precedence = sources.module_precedence.get(&extension.source).copied().unwrap_or_default();
    }
    extensions.sort_by(|left, right| right.precedence.cmp(&left.precedence).then_with(|| left.source.cmp(&right.source)));
    let mut tools = BTreeMap::new();
    let mut hooks = Vec::new();
    let mut commands = BTreeMap::new();
    for extension in extensions.iter_mut().filter(|extension| extension.active) {
        let mut conflict = None;
        for tool in &extension.tools {
            if tools.contains_key(&tool.name) {
                conflict = Some(format!("tool name conflict: {}", tool.name));
                break;
            }
        }
        for command in &extension.commands {
            if commands.contains_key(&command.name) {
                conflict = Some(format!("command name conflict: {}", command.name));
                break;
            }
        }
        if let Some(error) = conflict {
            extension.active = false;
            extension.health = "unhealthy".into();
            extension.error = Some(error);
            continue;
        }
        for tool in &extension.tools {
            tools.insert(tool.name.clone(), tool.clone());
        }
        for command in &extension.commands {
            commands.insert(command.name.clone(), command.clone());
        }
        let mut event_indices = BTreeMap::<String, usize>::new();
        for hook in &mut extension.hooks {
            let index = event_indices.entry(hook.event.clone()).or_default();
            hook.index = *index;
            *index += 1;
        }
        hooks.extend(extension.hooks.clone());
    }
    let declarative_inputs = sources
        .declarative
        .iter()
        .map(|capability| declarative::Input {
            kind: capability.kind,
            path: capability.path.clone(),
            precedence: capability.precedence,
        })
        .collect::<Vec<_>>();
    let declarative_runtime = declarative::load(&declarative_inputs);
    let agent_dir = materialize_agents(cwd, generation, &declarative_runtime.agents)?;
    let commands: Vec<CommandDescriptor> = commands.into_values().collect();
    let command_dir = materialize_commands(cwd, generation, &commands)?;
    Ok(Registry {
        generation,
        fingerprint: sources.fingerprint,
        extensions,
        tools,
        hooks,
        command_dir,
        declarative: sources.declarative,
        declarative_runtime,
        agent_dir,
        installed_plugins: sources.installed_plugins,
    })
}

fn retire_generated_dirs(previous: Option<&Arc<Registry>>, current: &Registry) {
    let Some(previous) = previous else {
        return;
    };
    for (label, path, retained) in [
        ("command", previous.command_dir.as_ref(), current.command_dir.as_ref()),
        ("agent", previous.agent_dir.as_ref(), current.agent_dir.as_ref()),
    ] {
        let Some(path) = path else {
            continue;
        };
        if retained == Some(path) {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("failed to retire extension {label} generation {}: {error}", path.display());
            }
        }
    }
}

fn current(cwd: &Path) -> Result<Arc<Registry>, String> {
    let key = canonical_key(cwd);
    let sources = source_set(cwd)?;
    let previous = REGISTRIES
        .read()
        .map_err(|_| "extension registry lock poisoned")?
        .get(&key)
        .cloned();
    if let Some(registry) = &previous {
        if registry.fingerprint == sources.fingerprint {
            return Ok(registry.clone());
        }
    }
    let generation = previous
        .as_ref()
        .map(|registry| registry.generation.saturating_add(1))
        .unwrap_or(1);
    let previous_command_registry = previous.clone();
    let built = match build_registry(cwd, sources, generation) {
        Ok(registry) => Arc::new(registry),
        Err(_) if previous.is_some() => return Ok(previous.expect("checked previous registry")),
        Err(error) => return Err(error),
    };
    REGISTRIES
        .write()
        .map_err(|_| "extension registry lock poisoned")?
        .insert(key, built.clone());
    retire_generated_dirs(previous_command_registry.as_ref(), &built);
    Ok(built)
}

pub(crate) fn reload(cwd: &Path) -> Result<ReloadReport, String> {
    let key = canonical_key(cwd);
    let sources = source_set(cwd)?;
    let previous = REGISTRIES
        .read()
        .map_err(|_| "extension registry lock poisoned")?
        .get(&key)
        .cloned();
    let generation = previous
        .as_ref()
        .map(|registry| registry.generation.saturating_add(1))
        .unwrap_or(1);
    let built = Arc::new(build_registry(cwd, sources, generation)?);
    let commands = built
        .extensions
        .iter()
        .filter(|extension| extension.active)
        .map(|extension| extension.commands.len())
        .sum();
    let capabilities = built
        .extensions
        .iter()
        .filter(|extension| extension.active)
        .map(|extension| extension.capabilities.len())
        .sum::<usize>()
        + built.declarative.iter().filter(|capability| capability.healthy && matches!(capability.kind, "commands" | "hooks")).count()
        + built.declarative_runtime.capabilities.iter().filter(|capability| capability.active).count();
    let report = ReloadReport {
        generation,
        active_extensions: built.extensions.iter().filter(|extension| extension.active).count(),
        unhealthy_extensions: built.extensions.iter().filter(|extension| !extension.active).count(),
        tools: built.tools.len(),
        commands,
        hooks: built.hooks.len(),
        capabilities,
    };
    REGISTRIES
        .write()
        .map_err(|_| "extension registry lock poisoned")?
        .insert(key, built.clone());
    retire_generated_dirs(previous.as_ref(), &built);
    crate::capability::invalidate();
    Ok(report)
}

pub(crate) fn capability_descriptors(cwd: &Path) -> Result<Vec<RegistryDescriptor>, String> {
    let registry = current(cwd)?;
    let mut out = Vec::new();
    for extension in &registry.extensions {
        let health = if extension.active {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable(extension.error.clone().unwrap_or_else(|| extension.health.clone()))
        };
        out.push(RegistryDescriptor::new(
            format!("extension/{}", extension.source.to_string_lossy().replace(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_' && ch != '.', "_")),
            CapabilityKind::Extension,
            extension.source.display().to_string(),
            extension.source.file_name().and_then(|name| name.to_str()).unwrap_or("extension"),
            format!("Extension module {}", extension.source.display()),
            FunctionTarget::Extension { source: extension.source.clone() },
        ).operation("activate").health(health.clone()));
        for tool in &extension.tools {
            out.push(RegistryDescriptor::new(
                format!("tool/{}", tool.name), CapabilityKind::Tool, extension.source.display().to_string(),
                tool.name.clone(), tool.description.clone(),
                FunctionTarget::ExtensionTool { name: tool.name.clone(), source: extension.source.clone() },
            ).operation("execute").policy(match tool.permission.as_deref() {
                Some("write") | Some("command") => CapabilityPolicy::ApprovalRequired,
                _ => CapabilityPolicy::Sandboxed,
            }).health(health.clone()).executable(tool.name.clone()).metadata(json!({"input": tool.input})));
        }
        for command in &extension.commands {
            out.push(RegistryDescriptor::new(
                format!("slash/{}", command.name), CapabilityKind::SlashCommand, extension.source.display().to_string(),
                command.name.clone(), command.description.clone(),
                FunctionTarget::FileSlash { command: command.name.clone(), path: registry.command_dir.clone().unwrap_or_default().join(format!("{}.md", command.name)) },
            ).operation("expand").policy(CapabilityPolicy::Sandboxed).health(health.clone()).executable(format!("/{}", command.name)));
        }
        for declared in &extension.capabilities {
            let kind = match declared.kind.as_str() {
                "skill" | "skills" => CapabilityKind::Skill,
                "agent" | "agents" => CapabilityKind::Agent,
                "rule" | "rules" => CapabilityKind::Rule,
                "service" | "services" => CapabilityKind::Service,
                _ => CapabilityKind::PluginContribution,
            };
            let mut descriptor = RegistryDescriptor::new(
                format!("{}/{}", declared.kind.trim_end_matches('s'), declared.id), kind,
                extension.source.display().to_string(), declared.id.clone(), declared.description.clone(),
                FunctionTarget::Extension { source: extension.source.clone() },
            ).operation("activate").health(health.clone());
            descriptor.version = declared.version.clone();
            out.push(descriptor);
        }
    }
    for plugin in &registry.installed_plugins {
        let active = registry.extensions.iter().any(|extension| {
            extension.active && extension.source.starts_with(&plugin.path)
        }) || registry.declarative_runtime.capabilities.iter().any(|capability| {
            capability.active && capability.path.starts_with(&plugin.path)
        }) || registry.declarative.iter().any(|capability| {
            capability.healthy
                && matches!(capability.kind, "commands" | "hooks")
                && capability.path.starts_with(&plugin.path)
        });
        let health = if !plugin.enabled {
            CapabilityHealth::disabled("plugin disabled by configuration")
        } else if active {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable("installed plugin has no successfully activated capabilities")
        };
        let mut descriptor = RegistryDescriptor::new(
            format!("plugin/{}", plugin.id), CapabilityKind::PluginContribution,
            plugin.path.display().to_string(), plugin.id.clone(), "Installed plugin contribution",
            FunctionTarget::Declarative { path: plugin.path.clone() },
        ).operation("discover").health(health).metadata(json!({"installed": true, "enabled": plugin.enabled, "active": active}));
        descriptor.version = plugin.version.clone();
        out.push(descriptor);
    }
    for capability in registry.declarative.iter().filter(|capability| matches!(capability.kind, "commands" | "hooks")) {
        let health = if capability.healthy {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable(capability.error.clone().unwrap_or_else(|| "declarative capability unavailable".into()))
        };
        out.push(RegistryDescriptor::new(
            format!("contribution/{}", capability.path.to_string_lossy().replace(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_' && ch != '.', "_")),
            CapabilityKind::PluginContribution, capability.path.display().to_string(),
            capability.path.file_name().and_then(|name| name.to_str()).unwrap_or(capability.kind),
            format!("Activated {} contribution", capability.kind), FunctionTarget::Declarative { path: capability.path.clone() },
        ).operation("load").policy(CapabilityPolicy::ReadOnly).health(health));
    }
    for capability in registry.declarative_runtime.capabilities.iter().filter(|capability| capability.active || !capability.healthy) {
        let kind = match capability.kind {
            "skill" => CapabilityKind::Skill,
            "agent" => CapabilityKind::Agent,
            "rule" => CapabilityKind::Rule,
            _ => CapabilityKind::PluginContribution,
        };
        let health = if capability.healthy {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable(capability.error.clone().unwrap_or_else(|| "definition activation failed".into()))
        };
        out.push(RegistryDescriptor::new(
            format!("{}/{}", capability.kind, capability.id), kind,
            capability.path.display().to_string(), capability.id.clone(), capability.description.clone(),
            FunctionTarget::Declarative { path: capability.path.clone() },
        ).operation("load").policy(CapabilityPolicy::ReadOnly).health(health).metadata(capability.metadata.clone()));
    }
    Ok(out)
}

pub(crate) fn tools(cwd: &Path) -> Result<Vec<ToolInfo>, String> {
    Ok(current(cwd)?
        .tools
        .values()
        .map(|tool| ToolInfo {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input: tool.input.clone(),
        })
        .collect())
}

pub(crate) fn command_dirs(cwd: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(current(cwd)?.command_dir.iter().cloned().collect())
}
pub(crate) fn agent_dirs(cwd: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(current(cwd)?.agent_dir.iter().cloned().collect())
}

pub(crate) fn model_entries(cwd: &Path) -> Vec<crate::control_plane::brama::ModelEntry> {
    let registry = match current(cwd) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("extension model registry unavailable: {error}");
            return Vec::new();
        }
    };
    let mut extensions = registry.extensions.iter().filter(|extension| extension.active).collect::<Vec<_>>();
    extensions.sort_by(|left, right| left.precedence.cmp(&right.precedence).then_with(|| left.source.cmp(&right.source)));
    extensions.into_iter().flat_map(|extension| extension.models.clone()).collect()
}

pub(crate) fn provider_entries(cwd: &Path) -> Vec<crate::control_plane::weles::Provider> {
    let registry = match current(cwd) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("extension provider registry unavailable: {error}");
            return Vec::new();
        }
    };
    let mut extensions = registry.extensions.iter().filter(|extension| extension.active).collect::<Vec<_>>();
    extensions.sort_by(|left, right| left.precedence.cmp(&right.precedence).then_with(|| left.source.cmp(&right.source)));
    extensions.into_iter().flat_map(|extension| extension.providers.clone()).collect()
}
pub(crate) fn prompt_context(
    cwd: &Path,
    prompt: &str,
) -> Result<Vec<declarative::PromptContribution>, String> {
    Ok(declarative::prompt_context(&current(cwd)?.declarative_runtime, prompt))
}

pub(crate) fn skill_context(
    cwd: &Path,
    skill_ids: &[String],
) -> Result<Vec<declarative::PromptContribution>, String> {
    declarative::skill_context(&current(cwd)?.declarative_runtime, skill_ids)
}

pub(crate) use declarative::PromptContribution;

pub(crate) fn execute_tool(
    cwd: &Path,
    artifact_dir: Option<&Path>,
    operation: &crate::tool_runtime::runtime_ops::OperationContext<'_>,
    allow_write: bool,
    allow_command: bool,
    name: &str,
    input: &Value,
) -> Result<Option<Value>, String> {
    let registry = current(cwd)?;
    let Some(tool) = registry.tools.get(name) else {
        return Ok(None);
    };
    let response = run_host(
        cwd,
        "execute_tool",
        registry.generation,
        INVOCATION_TIMEOUT,
        &[
            ("JEDEN_EXTENSION_SOURCE", tool.source.to_string_lossy().into_owned()),
            ("JEDEN_EXTENSION_TARGET", name.to_string()),
            ("JEDEN_EXTENSION_INPUT", input.to_string()),
        ],
        allow_write,
        allow_command,
        artifact_dir,
        Some(operation),
    )?;
    Ok(Some(response.get("result").cloned().unwrap_or(Value::Null)))
}

pub(crate) fn fire_hooks(
    cwd: &Path,
    event: &str,
    tool: &str,
    payload: &Value,
    allow_command: bool,
) -> Result<Vec<Value>, String> {
    let registry = current(cwd)?;
    let mut results = Vec::new();
    for hook in registry.hooks.iter().filter(|hook| {
        hook.event == event
            && (tool.is_empty()
                || hook.matcher.is_empty()
                || regex::Regex::new(&hook.matcher)
                    .map(|matcher| matcher.is_match(tool))
                    .unwrap_or(false))
    }) {
        let index = hook.index;
        let response = run_host(
            cwd,
            "fire_hook",
            registry.generation,
            INVOCATION_TIMEOUT,
            &[
                ("JEDEN_EXTENSION_SOURCE", hook.source.to_string_lossy().into_owned()),
                ("JEDEN_EXTENSION_EVENT", event.to_string()),
                ("JEDEN_EXTENSION_HOOK_INDEX", index.to_string()),
                ("JEDEN_EXTENSION_INPUT", payload.to_string()),
            ],
            false,
            allow_command,
            None,
            None,
        )?;
        results.push(response.get("result").cloned().unwrap_or(Value::Null));
    }
    Ok(results)
}

pub(crate) fn status(cwd: &Path) -> Result<String, String> {
    let registry = current(cwd)?;
    let mut lines = vec![format!(
        "Extension registry ABI {} generation {}: {} active, {} unhealthy",
        ABI_VERSION,
        registry.generation,
        registry.extensions.iter().filter(|extension| extension.active).count(),
        registry.extensions.iter().filter(|extension| !extension.active).count()
    )];
    for extension in &registry.extensions {
        lines.push(format!(
            "- {} [{}] tools={} commands={} hooks={} providers={} models={}{}",
            extension.source.display(),
            if extension.active { "active" } else { &extension.health },
            extension.tools.len(),
            extension.commands.len(),
            extension.hooks.len(),
            extension.providers.len(),
            extension.models.len(),
            extension.error.as_ref().map(|error| format!(": {error}")).unwrap_or_default()
        ));
    }
    for plugin in &registry.installed_plugins {
        let active = registry.extensions.iter().any(|extension| extension.active && extension.source.starts_with(&plugin.path))
            || registry.declarative_runtime.capabilities.iter().any(|capability| capability.active && capability.path.starts_with(&plugin.path))
            || registry.declarative.iter().any(|capability| capability.healthy && matches!(capability.kind, "commands" | "hooks") && capability.path.starts_with(&plugin.path));
        lines.push(format!(
            "- plugin {} version={} installed=yes enabled={} active={}",
            plugin.id, plugin.version, plugin.enabled, active
        ));
    }
    for capability in registry.declarative.iter().filter(|capability| matches!(capability.kind, "commands" | "hooks")) {
        lines.push(format!(
            "- {} {} [{}]{}",
            capability.kind,
            capability.path.display(),
            if capability.healthy { "active" } else { "unhealthy" },
            capability.error.as_ref().map(|error| format!(": {error}")).unwrap_or_default()
        ));
    }
    for capability in &registry.declarative_runtime.capabilities {
        lines.push(format!(
            "- {} {} ({}) [{}]{}",
            capability.kind,
            capability.id,
            capability.path.display(),
            if capability.active { "active" } else if capability.healthy { "shadowed" } else { "unhealthy" },
            capability.error.as_ref().map(|error| format!(": {error}")).unwrap_or_default()
        ));
    }
    Ok(lines.join("\n"))
}
