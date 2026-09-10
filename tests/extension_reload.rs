//! A tool asks this session to load extension material, and it happens here.
//!
//! Before this existed, an extension could install a new release and then only
//! report that the runtime it had installed was not the runtime it was
//! running: the host offered no reload request, and the sole way to load it was
//! a slash command typed by a person. The operator's answer to that was "nie,
//! nie bede nic ustawial. to Ty masz to naprawic".
//!
//! The test drives the real extension host process and the real tool
//! dispatcher, `jeden::tool_runtime::execute`, against a real extension file.
//!
//! Run: `cargo test --test extension_reload -- --nocapture`

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use jeden::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken, OperationContext};
use jeden::tool_runtime::{execute, ToolRuntime};

const ASKING_TOOL: &str = "reload_probe";
const QUIET_TOOL: &str = "quiet_probe";
const RELOAD_REQUEST: &str = ".jeden/runtime/extensions/reload-request.json";

fn fixture(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("extension-reload-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

fn write_extension(cwd: &Path) {
    let dir = cwd.join(".jeden/extensions");
    fs::create_dir_all(&dir).expect("create extension dir");
    fs::write(
        dir.join("probe.mjs"),
        format!(
            r#"export default function activate(api) {{
  api.registerTool({{
    name: '{ASKING_TOOL}',
    description: 'asks this session to load new extension material',
    parameters: api.zod.object({{}}),
    execute: async (_id, _input, _update, context) => {{
      const answer = await context.requestReload();
      return {{ asked: true, answer }};
    }},
  }});
  api.registerTool({{
    name: '{QUIET_TOOL}',
    description: 'answers without asking for anything',
    parameters: api.zod.object({{}}),
    execute: async () => ({{ asked: false }}),
  }});
}}
"#
        ),
    )
    .expect("write extension");
}

fn run(cwd: &Path, tool: &str) -> Value {
    let artifacts = ArtifactSink::new(cwd.join("artifacts"));
    let operation = OperationContext::new(CancellationToken::new(), artifacts);
    let runtime = ToolRuntime {
        cwd,
        artifact_dir: None,
        operation,
        allow_write: true,
        allow_command: false,
        interactive: false,
        ask_user: None,
    };
    execute(&runtime, tool, &json!({})).expect("the extension tool answers")
}

/// One test function: the extension loader reads `HOME` from the process
/// environment, so parallel tests would race each other over it.
#[test]
fn a_tool_can_make_this_session_load_new_extension_material() {
    let home = fixture("home");
    let cwd = fixture("cwd");
    env::set_var("HOME", &home);
    write_extension(&cwd);

    let quiet = run(&cwd, QUIET_TOOL);
    assert_eq!(
        quiet.get("extensionReload"),
        None,
        "a tool that asks for nothing must not carry a reload report: {quiet}"
    );
    assert!(
        !cwd.join(RELOAD_REQUEST).exists(),
        "no request file may be left behind by a quiet tool"
    );

    let asked = run(&cwd, ASKING_TOOL);
    let report = asked
        .get("extensionReload")
        .unwrap_or_else(|| panic!("the reload report must travel beside the answer: {asked}"));
    assert_eq!(
        report.get("reloaded"),
        Some(&json!(true)),
        "the reload must be reported as done, not merely requested: {report}"
    );
    assert!(
        report
            .get("generation")
            .and_then(Value::as_u64)
            .is_some_and(|generation| generation >= 1),
        "the reloaded registry must name its generation: {report}"
    );
    assert!(
        report
            .get("tools")
            .and_then(Value::as_u64)
            .is_some_and(|tools| tools >= 2),
        "the reloaded registry must still hold this extension's tools: {report}"
    );
    assert_eq!(
        asked.get("result").and_then(|value| value.get("asked")),
        Some(&json!(true)),
        "the tool's own answer must survive the reload report: {asked}"
    );
    assert!(
        !cwd.join(RELOAD_REQUEST).exists(),
        "the consumed request must not reload again on the next call"
    );

    // A second call reloads again: the request is per call, never sticky.
    let again = run(&cwd, ASKING_TOOL);
    assert_eq!(
        again
            .get("extensionReload")
            .and_then(|report| report.get("reloaded")),
        Some(&json!(true)),
        "a later call must be able to reload again: {again}"
    );
}
