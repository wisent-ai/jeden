use serde_json::{json, Value};
use std::path::Path;
use std::sync::{LazyLock, RwLock};

mod custom;
mod edit;
mod exec;
mod language;
mod read;
#[path = "../runtime_ops/mod.rs"]
pub mod runtime_ops;
mod session;
pub(crate) mod shared;

use custom::{
    custom_tool, mcp_call_tool, mcp_get_prompt, mcp_list_prompts, mcp_list_resources,
    mcp_list_tools, mcp_native_tool, mcp_read_resource,
};
use edit::{
    apply_patch_tool, delete_file, edit_file, move_file, visual_edit, write_any, write_archive,
    write_file, write_sqlite,
};
use exec::{
    delegate_task, eval_session, fetch_url, git_diff, git_log, git_show, git_status, glob_paths,
    grep_regex, list_package_scripts, node_eval, pty_resize, pty_session, python_eval, run_command,
    run_package_script, run_process, search_files, search_text,
};
use language::{ast_rewrite, ast_search, lsp};
use read::{
    fetch_readable_url, list_dir, read_any, read_archive, read_binary_file, read_document,
    read_file, read_image, read_sqlite,
};
use session::{
    ask_user, list_artifacts, memory_tool, read_artifact, recall_conversation, save_artifact,
    todo_tool,
};

#[derive(Clone, Debug)]
pub struct DynamicToolDescriptor {
    pub name: String,
    pub description: String,
    pub input: Value,
    pub healthy: bool,
    pub health: String,
}

pub type DynamicToolHandler =
    for<'a> fn(&ToolRuntime<'a>, &str, &Value) -> Option<Result<Value, String>>;
#[derive(Clone, Copy)]
pub struct DynamicToolRegistration {
    pub owner: &'static str,
    pub descriptors: fn() -> Vec<DynamicToolDescriptor>,
    pub execute: DynamicToolHandler,
}

static DYNAMIC_TOOLS: LazyLock<RwLock<Vec<DynamicToolRegistration>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

pub fn register_dynamic_tools(registration: DynamicToolRegistration) -> Result<(), String> {
    let proposed = (registration.descriptors)();
    if proposed
        .iter()
        .any(|descriptor| descriptor.name.trim().is_empty())
    {
        return Err(format!(
            "dynamic tool registration {} contains an empty name",
            registration.owner
        ));
    }
    let mut registrations = DYNAMIC_TOOLS
        .write()
        .map_err(|_| "dynamic tool registry poisoned")?;
    if registrations
        .iter()
        .any(|current| current.owner == registration.owner)
    {
        return Ok(());
    }
    for current in registrations.iter() {
        let existing = (current.descriptors)();
        if let Some(name) = proposed.iter().find_map(|candidate| {
            existing
                .iter()
                .find(|item| item.name == candidate.name)
                .map(|_| candidate.name.clone())
        }) {
            return Err(format!("dynamic tool name already registered: {name}"));
        }
    }
    registrations.push(registration);
    drop(registrations);
    crate::capability::invalidate();
    Ok(())
}

fn ensure_dynamic_registrations() {
    let _ = crate::task_runtime::register_with_tool_runtime();
    let _ = crate::tool_services::register_with_tool_runtime();
    let _ = register_dynamic_tools(language::registration());
}

pub fn dynamic_tool_descriptors() -> Vec<DynamicToolDescriptor> {
    ensure_dynamic_registrations();
    DYNAMIC_TOOLS
        .read()
        .map(|registrations| {
            registrations
                .iter()
                .flat_map(|registration| (registration.descriptors)())
                .filter(|descriptor| descriptor.healthy)
                .collect()
        })
        .unwrap_or_default()
}

fn execute_dynamic(
    runtime: &ToolRuntime<'_>,
    tool: &str,
    input: &Value,
) -> Option<Result<Value, String>> {
    let registrations = DYNAMIC_TOOLS.read().ok()?;
    registrations
        .iter()
        .find_map(|registration| (registration.execute)(runtime, tool, input))
}

pub struct ToolRuntime<'a> {
    pub cwd: &'a Path,
    pub artifact_dir: Option<&'a Path>,
    pub operation: runtime_ops::OperationContext<'a>,
    pub allow_write: bool,
    pub allow_command: bool,
    /// Cooked-terminal access for non-TUI execution.
    pub interactive: bool,
    /// Background TUI turns use this channel instead of reading stdin.
    pub ask_user: Option<&'a dyn Fn(&str, &[String]) -> Result<String, String>>,
}

pub(crate) fn tool_allowed_by_env(tool: &str) -> bool {
    let Some(raw) = std::env::var_os("JEDEN_AGENT_TOOLS") else {
        return true;
    };
    let allowed = raw.to_string_lossy();
    let mut any = false;
    let matched = allowed
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .any(|name| {
            any = true;
            name == tool
        });
    !any || matched
}

pub fn execute(runtime: &ToolRuntime<'_>, tool: &str, input: &Value) -> Result<Value, String> {
    ensure_dynamic_registrations();
    if !tool_allowed_by_env(tool) {
        return Err(format!("tool is not allowed for this agent: {tool}"));
    }
    match tool {
        "list_dir" => list_dir(runtime, input),
        "read" => read_any(runtime, input),
        "read_file" => read_file(runtime, input),
        "read_binary_file" => read_binary_file(runtime, input),
        "read_document" => read_document(runtime, input),
        "read_archive" => read_archive(runtime, input),
        "search_text" => search_text(runtime, input),
        "search_files" => search_files(runtime, input),
        "glob_paths" => glob_paths(runtime, input),
        "read_image" => read_image(runtime, input),
        "read_sqlite" => read_sqlite(runtime, input),
        "grep_regex" => grep_regex(runtime, input),
        "write_file" => write_file(runtime, input),
        "write" => write_any(runtime, input),
        "write_archive" => write_archive(runtime, input),
        "write_sqlite" => write_sqlite(runtime, input),
        "apply_patch" => apply_patch_tool(runtime, input),
        "edit_file" => edit_file(runtime, input),
        "edit" => visual_edit(runtime, input),
        "delete_file" => delete_file(runtime, input),
        "move_file" => move_file(runtime, input),
        "run_command" => run_command(runtime, input),
        "run_process" => run_process(runtime, input),
        "node_eval" => node_eval(runtime, input),
        "python_eval" => python_eval(runtime, input),
        "eval_session" => eval_session(runtime, input),
        "pty_session" => pty_session(runtime, input),
        "pty_resize" => pty_resize(runtime, input),
        "ast_search" => ast_search(runtime, input),
        "ast_rewrite" => ast_rewrite(runtime, input),
        "lsp" => lsp(runtime, input),
        "list_package_scripts" => list_package_scripts(runtime),
        "run_package_script" => run_package_script(runtime, input),
        "git_status" => git_status(runtime),
        "git_diff" => git_diff(runtime, input),
        "todo" => todo_tool(runtime, input),
        "delegate_task" => delegate_task(runtime, input),
        "git_log" => git_log(runtime, input),
        "git_show" => git_show(runtime, input),
        "fetch_url" => fetch_url(runtime, input),
        "fetch_readable_url" => fetch_readable_url(runtime, input),
        "save_artifact" => save_artifact(runtime, input),
        "list_artifacts" => list_artifacts(runtime),
        "read_artifact" => read_artifact(runtime, input),
        "recall_conversation" => recall_conversation(runtime, input),
        "memory" => memory_tool(runtime, input),
        "ask_user" => ask_user(runtime, input),
        "mcp_list_tools" => mcp_list_tools(runtime, input),
        "mcp_call_tool" => mcp_call_tool(runtime, input),
        "mcp_list_resources" => mcp_list_resources(runtime, input),
        "mcp_read_resource" => mcp_read_resource(runtime, input),
        "mcp_list_prompts" => mcp_list_prompts(runtime, input),
        "mcp_get_prompt" => mcp_get_prompt(runtime, input),
        other => {
            if let Some(result) = execute_dynamic(runtime, other, input) {
                result
            } else if let Some(result) = mcp_native_tool(runtime, other, input) {
                result
            } else {
                custom_tool(runtime, other, input)
            }
        }
    }
}

pub fn format_tool_result(result: &Value) -> String {
    json!({"type": "tool_result", "result": result}).to_string()
}
