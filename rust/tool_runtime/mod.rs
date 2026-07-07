use serde_json::{json, Value};
use std::path::Path;

mod custom;
mod edit;
mod exec;
mod read;
mod session;
mod shared;

use custom::{custom_tool, mcp_call_tool, mcp_get_prompt, mcp_list_prompts, mcp_list_resources, mcp_list_tools, mcp_native_tool, mcp_read_resource};
use edit::{apply_patch_tool, delete_file, edit_file, move_file, visual_edit, write_file};
use exec::{delegate_task, fetch_url, git_diff, git_log, git_show, git_status, glob_paths, grep_regex, list_package_scripts, node_eval, python_eval, run_command, run_package_script, run_process, search_files, search_text};
use read::{fetch_readable_url, list_dir, read_archive, read_binary_file, read_document, read_file, read_image, read_sqlite};
use session::{ask_user, list_artifacts, memory_tool, read_artifact, recall_conversation, save_artifact, todo_tool};

#[derive(Debug, Clone)]
pub struct ToolRuntime<'a> {
    pub cwd: &'a Path,
    pub artifact_dir: Option<&'a Path>,
    pub allow_write: bool,
    pub allow_command: bool,
    /// False when the turn runs on a background thread while the TUI owns the
    /// terminal; stdin-reading tools (ask_user) must refuse instead of stealing
    /// keystrokes from the interactive event loop.
    pub interactive: bool,
}

pub fn execute(runtime: &ToolRuntime<'_>, tool: &str, input: &Value) -> Result<Value, String> {
    match tool {
        "list_dir" => list_dir(runtime, input),
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
        "apply_patch" => apply_patch_tool(runtime, input),
        "edit_file" => edit_file(runtime, input),
        "edit" => visual_edit(runtime, input),
        "delete_file" => delete_file(runtime, input),
        "move_file" => move_file(runtime, input),
        "run_command" => run_command(runtime, input),
        "run_process" => run_process(runtime, input),
        "node_eval" => node_eval(runtime, input),
        "python_eval" => python_eval(runtime, input),
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
            if let Some(result) = mcp_native_tool(runtime, other, input) { result } else { custom_tool(runtime, other, input) }
        }
    }
}

pub fn format_tool_result(result: &Value) -> String {
    json!({"type": "tool_result", "result": result}).to_string()
}
