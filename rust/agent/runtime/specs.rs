use super::*;

pub(in crate::agent) fn rust_tool_specs(cwd: &Path) -> Vec<Value> {
    let mut specs = vec![
        tool_spec("list_dir", "List a directory under cwd", json!({"path": {"type": "string"}, "limit": {"type": "number"}}), vec![]),
        tool_spec("read_file", "Read a UTF-8 file under cwd", json!({"path": {"type": "string"}}), vec!["path"]),
        tool_spec("read_binary_file", "Read one binary file under cwd as base64", json!({"path": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["path"]),
        tool_spec("read_document", "Extract readable text from one document under cwd with optional line range", json!({"path": {"type": "string"}, "maxBytes": {"type": "number"}, "range": {"type": "string"}}), vec!["path"]),
        tool_spec("read_archive", "List archive entries or read one entry from .zip, .tar, .tar.gz, or .tgz under cwd", json!({"path": {"type": "string"}, "entry": {"type": "string"}, "mode": {"type": "string"}, "maxBytes": {"type": "number"}, "range": {"type": "string"}}), vec!["path"]),
        tool_spec("read_image", "Read one PNG, JPEG, GIF, or WebP image under cwd as base64 with mime type and dimensions", json!({"path": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["path"]),
        tool_spec("read_sqlite", "Read a SQLite database under cwd: list tables, inspect a table, fetch one row, or run a read-only SELECT/WITH query", json!({"path": {"type": "string"}, "table": {"type": "string"}, "key": {"type": "string"}, "query": {"type": "string"}, "limit": {"type": "number"}, "offset": {"type": "number"}, "where": {"type": "string"}, "order": {"type": "string"}}), vec!["path"]),
        tool_spec("search_text", "Search one file for a literal string", json!({"path": {"type": "string"}, "query": {"type": "string"}, "caseSensitive": {"type": "boolean"}}), vec!["path", "query"]),
        tool_spec("search_files", "Recursively search text files under cwd for a literal string", json!({"path": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}}, "query": {"type": "string"}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "caseSensitive": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec!["query"]),
        tool_spec("glob_paths", "Find files under cwd with simple glob patterns", json!({"patterns": {"type": "string"}, "path": {"type": "string"}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec![]),
        tool_spec("grep_regex", "Search text files under cwd with a regular expression", json!({"expr": {"type": "string"}, "path": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "multiline": {"type": "boolean"}, "caseSensitive": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec!["expr"]),
        tool_spec("write_file", "Create or overwrite a UTF-8 file under cwd; overwrites require expectedSha256 and write-tier approval", json!({"path": {"type": "string"}, "content": {"type": "string"}, "expectedSha256": {"type": "string"}}), vec!["path", "content"]),
        tool_spec("apply_patch", "Apply exact one-occurrence string replacements to an existing UTF-8 file; requires expectedSha256 and write-tier approval", json!({"path": {"type": "string"}, "expectedSha256": {"type": "string"}, "replacements": {"type": "array"}}), vec!["path", "expectedSha256", "replacements"]),
        tool_spec("edit_file", "Apply line-based edits to a UTF-8 file under cwd; requires expectedSha256 and write-tier approval", json!({"path": {"type": "string"}, "expectedSha256": {"type": "string"}, "ops": {"type": "array"}}), vec!["path", "expectedSha256", "ops"]),
        tool_spec("edit", "Apply an OMP-style anchored visual patch string with [path#TAG], SWAP/DEL/INS/REM/MV and safe block hunks; requires write-tier approval", json!({"patch": {"type": "string"}}), vec!["patch"]),
        tool_spec("delete_file", "Delete one file under cwd; requires expectedSha256 and write-tier approval", json!({"path": {"type": "string"}, "expectedSha256": {"type": "string"}}), vec!["path", "expectedSha256"]),
        tool_spec("move_file", "Move or rename one file under cwd; requires expectedSha256 and write-tier approval", json!({"from": {"type": "string"}, "to": {"type": "string"}, "expectedSha256": {"type": "string"}, "overwrite": {"type": "boolean"}}), vec!["from", "to", "expectedSha256"]),
        tool_spec("run_command", "Run a shell command in cwd; requires exec-tier approval", json!({"command": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["command"]),
        tool_spec("run_process", "Run one process with argv array in cwd; requires exec-tier approval", json!({"command": {"type": "string"}, "args": {"type": "array", "items": {"type": "string"}}, "stdin": {"type": "string"}, "timeoutMs": {"type": "number"}, "env": {"type": "object"}}), vec!["command"]),
        tool_spec("node_eval", "Run JavaScript with node --input-type=module in cwd; requires exec-tier approval", json!({"code": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["code"]),
        tool_spec("python_eval", "Run Python code with python3 in cwd; requires exec-tier approval", json!({"code": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["code"]),
        tool_spec("list_package_scripts", "List package.json scripts in cwd", json!({}), vec![]),
        tool_spec("run_package_script", "Run one existing package.json script with npm; requires exec-tier approval", json!({"script": {"type": "string"}, "timeoutMs": {"type": "number"}, "env": {"type": "object"}}), vec!["script"]),
        tool_spec("git_status", "Read git status --short for cwd", json!({}), vec![]),
        tool_spec("git_diff", "Read git diff for cwd or one path under cwd", json!({"path": {"type": "string"}}), vec![]),
        tool_spec("git_log", "Read recent git commits for cwd or one path under cwd", json!({"limit": {"type": "number"}, "path": {"type": "string"}}), vec![]),
        tool_spec("git_show", "Read one git object or commit summary", json!({"ref": {"type": "string"}, "path": {"type": "string"}}), vec![]),
        tool_spec("fetch_url", "Fetch one HTTP(S) URL and return capped text; supports optional line range", json!({"url": {"type": "string"}, "maxBytes": {"type": "number"}, "timeoutMs": {"type": "number"}, "range": {"type": "string"}}), vec!["url"]),
        tool_spec("fetch_readable_url", "Fetch one HTTP(S) URL and return simplified readable text with optional line range", json!({"url": {"type": "string"}, "maxBytes": {"type": "number"}, "timeoutMs": {"type": "number"}, "range": {"type": "string"}}), vec!["url"]),
        tool_spec("save_artifact", "Save UTF-8 content into the current session artifacts directory", json!({"name": {"type": "string"}, "content": {"type": "string"}}), vec!["content"]),
        tool_spec("list_artifacts", "List files in the current session artifact directory", json!({}), vec![]),
        tool_spec("read_artifact", "Read one UTF-8 artifact from the current session artifact directory", json!({"name": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["name"]),
        tool_spec("recall_conversation", "Return the text-only transcript of a recorded session (user prompts and final answers; tool calls, results, and images stripped); defaults to the current session, or pass session=<id-or-path>", json!({"session": {"type": "string"}}), vec![]),
        tool_spec("ask_user", "Ask the human user a question during an interactive session", json!({"question": {"type": "string"}, "options": {"type": "array"}}), vec!["question"]),
        tool_spec("todo", "Manage the current session todo list with init, append, start, done, drop, rm, and view operations", json!({"op": {"type": "string"}, "list": {"type": "array"}, "phase": {"type": "string"}, "items": {"type": "array"}, "task": {"type": "string"}}), vec!["op"]),
        tool_spec("delegate_task", "Run a focused subtask in a fresh Jeden session and return its result; requires exec-tier approval", json!({"task": {"type": "string"}, "maxSteps": {"type": "number"}}), vec!["task"]),
        tool_spec("memory", "Remember and recall durable scoped notes across Jeden sessions", json!({"op": {"type": "string"}, "text": {"type": "string"}, "query": {"type": "string"}, "tags": {"type": "array"}, "limit": {"type": "number"}, "kind": {"type": "string"}, "scope": {"type": "object"}, "confidence": {"type": "number"}}), vec!["op"]),
        tool_spec("mcp_list_tools", "List tools from a configured stdio MCP server", json!({"server": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server"]),
        tool_spec("mcp_call_tool", "Call one tool on a configured stdio MCP server", json!({"server": {"type": "string"}, "tool": {"type": "string"}, "args": {"type": "object"}, "timeoutMs": {"type": "number"}}), vec!["server", "tool"]),
        tool_spec("mcp_list_resources", "List resources from a configured stdio MCP server", json!({"server": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server"]),
        tool_spec("mcp_read_resource", "Read one resource from a configured stdio MCP server", json!({"server": {"type": "string"}, "uri": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server", "uri"]),
        tool_spec("mcp_list_prompts", "List prompts from a configured stdio MCP server", json!({"server": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server"]),
        tool_spec("mcp_get_prompt", "Get one prompt from a configured stdio MCP server", json!({"server": {"type": "string"}, "name": {"type": "string"}, "args": {"type": "object"}, "timeoutMs": {"type": "number"}}), vec!["server", "name"]),
    ];
    let mut seen = specs
        .iter()
        .filter_map(|spec| spec.get("function").and_then(|f| f.get("name")).and_then(Value::as_str).map(ToString::to_string))
        .collect::<BTreeSet<_>>();
    for tool in crate::tools::list_tools(cwd) {
        if seen.contains(&tool.name) {
            continue;
        }
        let parameters = if tool.input.get("type").is_some() {
            tool.input.clone()
        } else {
            json!({"type": "object", "properties": tool.input})
        };
        specs.push(json!({
            "type": "function",
            "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": parameters,
            }
        }));
        seen.insert(specs.last().and_then(|spec| spec.get("function")).and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or("").to_string());
    }
    specs
}

fn tool_spec(name: &str, description: &str, properties: Value, required: Vec<&str>) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            }
        }
    })
}

pub(in crate::agent) fn system_prompt(cwd: &Path) -> String {
    let tools = crate::tools::list_tools(cwd)
        .into_iter()
        .map(|tool| format!("- {}: {}", tool.name, tool.description))
        .collect::<Vec<_>>()
        .join("\n");
    let memory = memory_guidance_for_prompt(cwd)
        .map(|summary| format!("\n\nMemory Guidance (heuristic; verify against current repo before acting):\n{}", summary))
        .unwrap_or_default();
    format!("You are Jeden, Wisent's private agent harness.\n\nRules:\n- Answer with {{\"action\":\"final\",\"text\":\"your concise answer\"}} when done.\n- Use tool calls when the model-router supports native tool_calls, or answer with {{\"action\":\"tool\",\"tool\":\"tool_name\",\"input\":{{...}}}}.\n- Do not create tests unless the user explicitly asks.\n- Do not create docs unless the user explicitly asks.\n- Do not invent files, command outputs, or tool results.\n- Tool approval uses read/write/exec tiers. Write-tier tools mutate files or session state; exec-tier tools run code/processes or spawn agents. The active /approval policy, --allow-* flags, and --yolo decide whether a call runs or prompts.{}\n\nExecutable Rust tools:\n{}", memory, tools)
}
