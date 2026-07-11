use super::*;
use crate::capability::HealthState;
use crate::control_plane::{brama::BramaClient, weles::WelesClient};
use crate::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken, OperationContext};
use parking_lot::{Mutex, MutexGuard};
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    _guard: MutexGuard<'static, ()>,
    root: PathBuf,
    cwd: PathBuf,
    home: PathBuf,
    old_home: Option<std::ffi::OsString>,
    old_plugins_home: Option<std::ffi::OsString>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let guard = ENV_LOCK.lock();
        let root = std::env::temp_dir().join(format!(
            "jeden-extension-runtime-{name}-{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let cwd = root.join("project");
        let home = root.join("home");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&home).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_plugins_home = std::env::var_os("JEDEN_PLUGINS_HOME");
        std::env::set_var("HOME", &home);
        std::env::set_var("JEDEN_PLUGINS_HOME", &home);
        Self { _guard: guard, root, cwd, home, old_home, old_plugins_home }
    }

    fn extension(&self, relative: &str, source: &str) -> PathBuf {
        let path = self.cwd.join(".jeden/extensions").join(relative);
        write_file(&path, source);
        path
    }

    fn operation(&self) -> OperationContext<'static> {
        OperationContext::new(
            CancellationToken::new(),
            ArtifactSink::new(self.root.join("operation-artifacts")),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        restore_env("HOME", self.old_home.take());
        restore_env("JEDEN_PLUGINS_HOME", self.old_plugins_home.take());
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
    if let Some(value) = value {
        std::env::set_var(key, value);
    } else {
        std::env::remove_var(key);
    }
}

fn write_file(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn execute(cwd: &Path, operation: &OperationContext<'_>, allow_write: bool, name: &str, input: Value) -> Result<Option<Value>, String> {
    execute_tool(cwd, None, operation, allow_write, false, name, &input)
}

fn serve_once(body: &'static str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    (format!("http://{address}"), handle)
}

#[test]
fn extension_runtime_reload_advances_generation_and_replaces_tool_command_and_hook_behavior() {
    let fixture = Fixture::new("reload");
    let module = fixture.extension(
        "reload.mjs",
        r#"export default api => {
  api.registerTool({name:'generation-tool', description:'generation behavior', execute: input => ({version:'v1', value:input.value})});
  api.registerCommand({name:'generation-command', description:'v1 command', prompt:'prompt-v1'});
  api.on('PreToolUse', '^allowed$', payload => ({hook:'v1', payload}));
}"#,
    );

    let first = reload(&fixture.cwd).unwrap();
    assert_eq!((first.generation, first.active_extensions, first.tools, first.commands, first.hooks), (1, 1, 1, 1, 1));
    let first_command_dir = command_dirs(&fixture.cwd).unwrap().pop().unwrap();
    assert_eq!(fs::read_to_string(first_command_dir.join("generation-command.md")).unwrap(), "---\ndescription: \"v1 command\"\n---\nprompt-v1");
    assert_eq!(
        execute(&fixture.cwd, &fixture.operation(), false, "generation-tool", json!({"value": 7})).unwrap().unwrap(),
        json!({"version":"v1", "value":7})
    );
    assert_eq!(fire_hooks(&fixture.cwd, "PreToolUse", "blocked", &json!({"n":1}), false).unwrap(), Vec::<Value>::new());
    assert_eq!(fire_hooks(&fixture.cwd, "PreToolUse", "allowed", &json!({"n":1}), false).unwrap(), vec![json!({"hook":"v1", "payload":{"n":1}})]);

    write_file(
        &module,
        r#"export default api => {
  api.registerTool({name:'generation-tool', description:'generation behavior', execute: input => ({version:'v2', doubled:input.value * 2})});
  api.registerCommand({name:'generation-command', description:'v2 command', prompt:'prompt-v2'});
  api.on('PreToolUse', '^allowed$', payload => ({hook:'v2', seen:payload.n}));
}"#,
    );
    let second = reload(&fixture.cwd).unwrap();
    assert_eq!(second.generation, 2);
    let second_command_dir = command_dirs(&fixture.cwd).unwrap().pop().unwrap();
    assert_ne!(second_command_dir, first_command_dir);
    assert!(!first_command_dir.exists(), "the retired generation remained callable on disk");
    assert_eq!(fs::read_to_string(second_command_dir.join("generation-command.md")).unwrap(), "---\ndescription: \"v2 command\"\n---\nprompt-v2");
    assert_eq!(
        execute(&fixture.cwd, &fixture.operation(), false, "generation-tool", json!({"value": 7})).unwrap().unwrap(),
        json!({"version":"v2", "doubled":14})
    );
    assert_eq!(fire_hooks(&fixture.cwd, "PreToolUse", "allowed", &json!({"n":2}), false).unwrap(), vec![json!({"hook":"v2", "seen":2})]);
}

#[test]
fn extension_runtime_project_precedence_isolated_per_workspace_and_throwing_sibling_survives() {
    let fixture = Fixture::new("precedence");
    let plugin = fixture.root.join("plugin");
    write_file(
        &plugin.join("extensions/plugin.mjs"),
        "export default {name:'shared-tool',description:'plugin',execute:()=>({source:'plugin'})};",
    );
    write_file(
        &fixture.home.join(".jeden/extensions/user.mjs"),
        "export default {name:'shared-tool',description:'user',execute:()=>({source:'user'})};",
    );
    fixture.extension(
        "project.mjs",
        "export default {name:'shared-tool',description:'project',execute:()=>({source:'project'})};",
    );
    fixture.extension("bad.mjs", "throw new Error('activation exploded');");
    fixture.extension(
        "healthy.mjs",
        "export default {name:'healthy-tool',description:'healthy sibling',execute:()=>({healthy:true})};",
    );
    write_file(
        &fixture.home.join(".jeden/plugins.json"),
        &json!({"installed":{"fixture":{"id":"fixture","version":"1","path":plugin,"enabled":true}}}).to_string(),
    );

    let report = reload(&fixture.cwd).unwrap();
    assert_eq!((report.active_extensions, report.unhealthy_extensions, report.tools), (2, 3, 2));
    assert_eq!(
        execute(&fixture.cwd, &fixture.operation(), false, "shared-tool", json!({})).unwrap().unwrap(),
        json!({"source":"project"})
    );
    assert_eq!(
        execute(&fixture.cwd, &fixture.operation(), false, "healthy-tool", json!({})).unwrap().unwrap(),
        json!({"healthy":true})
    );
    let state = status(&fixture.cwd).unwrap();
    assert!(state.contains("activation exploded"));
    assert_eq!(state.matches("tool name conflict: shared-tool").count(), 2);

    let other = fixture.root.join("other-project");
    write_file(
        &other.join(".jeden/extensions/other.mjs"),
        "export default {name:'shared-tool',description:'other workspace',execute:()=>({source:'other'})};",
    );
    reload(&other).unwrap();
    assert_eq!(
        execute(&other, &fixture.operation(), false, "shared-tool", json!({})).unwrap().unwrap(),
        json!({"source":"other"})
    );
    assert_eq!(
        execute(&fixture.cwd, &fixture.operation(), false, "shared-tool", json!({})).unwrap().unwrap(),
        json!({"source":"project"})
    );
}

#[test]
fn extension_runtime_provider_and_model_registrations_are_consumed_by_control_plane_exports() {
    let fixture = Fixture::new("control-plane");
    fixture.extension(
        "catalog.mjs",
        r#"export default api => {
  api.registerProvider({id:'shared-provider', displayName:'Extension Provider', loginMethods:['api_key']});
  api.registerProvider({id:'extension-only', displayName:'Extension Only', loginMethods:['paste']});
  api.registerModel({id:'shared-model', contextWindow:64000, maxOutputTokens:4096, tools:true});
  api.registerModel({id:'extension-only-model', contextWindow:32000, maxOutputTokens:2048});
}"#,
    );
    reload(&fixture.cwd).unwrap();

    let (weles_url, weles_server) = serve_once(r#"{"providers":[{"id":"shared-provider","displayName":"Base Provider","loginMethods":[]},{"id":"base-only","displayName":"Base Only","loginMethods":[]}]}"#);
    let providers = crate::control_plane::providers(
        &fixture.cwd,
        &WelesClient::new(Some(weles_url), None, Duration::from_millis(1)),
    )
    .unwrap();
    weles_server.join().unwrap();
    assert_eq!(providers.iter().map(|provider| (provider.id.as_str(), provider.display_name.as_str())).collect::<Vec<_>>(), vec![
        ("base-only", "Base Only"),
        ("extension-only", "Extension Only"),
        ("shared-provider", "Extension Provider"),
    ]);

    let (brama_url, brama_server) = serve_once(r#"{"version":"v1","models":[{"id":"shared-model","contextWindow":1},{"id":"base-only-model","contextWindow":2}]}"#);
    let catalog = crate::control_plane::model_catalog(
        &fixture.cwd,
        &BramaClient::new(Some(brama_url), None, Duration::from_secs(60)),
        true,
    )
    .unwrap();
    brama_server.join().unwrap();
    assert_eq!(catalog.models.iter().map(|model| (model.id.as_str(), model.context_window)).collect::<Vec<_>>(), vec![
        ("base-only-model", 2),
        ("extension-only-model", 32_000),
        ("shared-model", 64_000),
    ]);
}

#[test]
fn extension_runtime_disable_then_uninstall_retires_generated_state_and_registration() {
    let fixture = Fixture::new("teardown");
    let plugin = fixture.root.join("installed-plugin");
    write_file(
        &plugin.join("extensions/index.mjs"),
        r#"export default api => {
  api.registerTool({name:'installed-tool',description:'installed',execute:()=>({installed:true})});
  api.registerCommand({name:'installed-command',prompt:'installed prompt'});
}"#,
    );
    let registry = fixture.home.join(".jeden/plugins.json");
    let installed = |enabled| json!({"installed":{"teardown":{"id":"teardown","version":"1","path":plugin,"enabled":enabled}}}).to_string();
    write_file(&registry, &installed(true));

    reload(&fixture.cwd).unwrap();
    let generated = command_dirs(&fixture.cwd).unwrap().pop().unwrap();
    assert!(generated.join("installed-command.md").exists());
    assert!(execute(&fixture.cwd, &fixture.operation(), false, "installed-tool", json!({})).unwrap().is_some());

    write_file(&registry, &installed(false));
    let disabled = reload(&fixture.cwd).unwrap();
    assert_eq!((disabled.generation, disabled.tools, disabled.commands), (2, 0, 0));
    assert!(!generated.exists());
    assert!(command_dirs(&fixture.cwd).unwrap().is_empty());
    assert!(execute(&fixture.cwd, &fixture.operation(), false, "installed-tool", json!({})).unwrap().is_none());
    let descriptors = capability_descriptors(&fixture.cwd).unwrap();
    let plugin_descriptor = descriptors.iter().find(|descriptor| descriptor.id == "plugin/teardown").unwrap();
    assert_eq!(plugin_descriptor.health.state, HealthState::Disabled);

    write_file(&registry, r#"{"installed":{}}"#);
    let uninstalled = reload(&fixture.cwd).unwrap();
    assert_eq!(uninstalled.generation, 3);
    assert!(!capability_descriptors(&fixture.cwd).unwrap().iter().any(|descriptor| descriptor.id == "plugin/teardown"));
}

#[test]
fn extension_runtime_enforces_cancellation_deadline_progress_and_artifact_jail() {
    let fixture = Fixture::new("operation-boundaries");
    fixture.extension(
        "operations.mjs",
        r#"export default api => {
  api.registerTool({name:'long-tool',description:'long operation',execute:async (_id,_input,update)=>{update({phase:'started'});await new Promise(resolve=>setTimeout(resolve,30000));return {finished:true};}});
  api.registerTool({name:'artifact-tool',description:'artifact operation',permission:'write',execute:async (_id,input,_update,ctx)=>({path:await ctx.artifact(input.name,input.content)})});
  api.registerTool({name:'read-tool',description:'jailed read',execute:async (_id,input,_update,ctx)=>({text:await api.readText(input.path)})});
}"#,
    );
    reload(&fixture.cwd).unwrap();

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let cancelled_context = OperationContext::new(cancelled, ArtifactSink::new(fixture.root.join("cancelled-artifacts")));
    assert_eq!(execute(&fixture.cwd, &cancelled_context, false, "artifact-tool", json!({"name":"x","content":"x"})).unwrap_err(), "extension operation cancelled");

    let deadline_context = OperationContext::new(CancellationToken::new(), ArtifactSink::new(fixture.root.join("deadline-artifacts")))
        .with_deadline(Instant::now() + Duration::from_millis(100));
    let deadline_error = execute(&fixture.cwd, &deadline_context, false, "long-tool", json!({})).unwrap_err();
    assert!(deadline_error.contains("timed out"), "unexpected deadline error: {deadline_error}");

    let token = CancellationToken::new();
    let progress_events = Arc::new(AtomicU64::new(0));
    let cancel_on_progress = token.clone();
    let observed = Arc::clone(&progress_events);
    let running_context = OperationContext::new(token, ArtifactSink::new(fixture.root.join("running-artifacts")))
        .with_progress(Arc::new(move |event| {
            assert_eq!(event.stream, "extension-stdout");
            assert!(event.bytes > 0);
            observed.fetch_add(1, Ordering::Relaxed);
            cancel_on_progress.cancel();
        }));
    assert_eq!(execute(&fixture.cwd, &running_context, false, "long-tool", json!({})).unwrap_err(), "extension operation cancelled");
    assert!(progress_events.load(Ordering::Relaxed) > 0);

    let artifact_root = fixture.root.join("jailed-artifacts");
    let artifact_context = OperationContext::new(CancellationToken::new(), ArtifactSink::new(&artifact_root));
    let denied = execute(&fixture.cwd, &artifact_context, false, "artifact-tool", json!({"name":"proof.txt","content":"proof"})).unwrap_err();
    assert!(denied.contains("requires --allow-write"));
    let escaped = execute(&fixture.cwd, &artifact_context, true, "artifact-tool", json!({"name":"../escape.txt","content":"escape"})).unwrap_err();
    assert!(escaped.contains("invalid artifact name"));
    assert!(!fixture.root.join("escape.txt").exists());
    let written = execute(&fixture.cwd, &artifact_context, true, "artifact-tool", json!({"name":"proof.txt","content":"proof"})).unwrap().unwrap();
    assert_eq!(fs::read_to_string(artifact_root.join("proof.txt")).unwrap(), "proof");
    assert_eq!(PathBuf::from(written["path"].as_str().unwrap()), artifact_root.join("proof.txt"));
    let duplicate = execute(&fixture.cwd, &artifact_context, true, "artifact-tool", json!({"name":"proof.txt","content":"replacement"})).unwrap_err();
    assert!(duplicate.contains("exist"));
    assert_eq!(fs::read_to_string(artifact_root.join("proof.txt")).unwrap(), "proof");

    write_file(&fixture.root.join("outside.txt"), "secret");
    let read_escape = execute(&fixture.cwd, &artifact_context, false, "read-tool", json!({"path":"../outside.txt"})).unwrap_err();
    assert!(read_escape.contains("path escapes root"));
}

#[test]
fn extension_runtime_activates_skills_rules_and_agents_for_downstream_consumers() {
    let fixture = Fixture::new("declarative");
    write_file(
        &fixture.cwd.join(".jeden/skills/reviewer/SKILL.md"),
        "---\nid: reviewer\ndescription: Review changes\nmatch: review|audit\n---\nUse the reviewer checklist.",
    );
    write_file(
        &fixture.cwd.join(".jeden/rules/security.md"),
        "---\nid: secure-output\nalwaysApply: true\n---\nNever expose secrets.",
    );
    write_file(
        &fixture.cwd.join(".jeden/agents/reviewer.json"),
        r#"{"id":"review-agent","description":"Reviews changes","prompt":"Review carefully","skills":["reviewer"],"tools":["read_file"]}"#,
    );

    let report = reload(&fixture.cwd).unwrap();
    assert_eq!(report.unhealthy_extensions, 0);
    let context = prompt_context(&fixture.cwd, "please audit this patch").unwrap();
    assert_eq!(context.iter().map(|item| (item.kind, item.id.as_str(), item.content.as_str())).collect::<Vec<_>>(), vec![
        ("rule", "secure-output", "Never expose secrets."),
        ("skill", "reviewer", "Use the reviewer checklist."),
    ]);
    let explicit = skill_context(&fixture.cwd, &["reviewer".to_string()]).unwrap();
    assert_eq!(explicit.len(), 1);
    assert_eq!((explicit[0].id.as_str(), explicit[0].content.as_str()), ("reviewer", "Use the reviewer checklist."));
    assert_eq!(skill_context(&fixture.cwd, &["missing".to_string()]).unwrap_err(), "active skill not found: missing");

    let agent_dir = agent_dirs(&fixture.cwd).unwrap().pop().unwrap();
    let materialized: Value = serde_json::from_slice(&fs::read(agent_dir.join("review-agent.json")).unwrap()).unwrap();
    assert_eq!(materialized["prompt"], "Review carefully");
    assert_eq!(materialized["skills"], json!(["reviewer"]));
    let descriptors = capability_descriptors(&fixture.cwd).unwrap();
    for id in ["skill/reviewer", "rule/secure-output", "agent/review-agent"] {
        let descriptor = descriptors.iter().find(|descriptor| descriptor.id == id).unwrap_or_else(|| panic!("missing downstream descriptor {id}"));
        assert_eq!(descriptor.health.state, HealthState::Healthy);
    }
}
