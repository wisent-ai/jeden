use rand::{distributions::Alphanumeric, Rng};
use serde_json::{json, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model_router::{chat_completion, ChatConfig};
use crate::protocol::{extract_json_object, parse_action, Action, ToolAction};
use crate::{handle_slash, load_config, session_root, Args, Config};

#[derive(Debug, Clone)]
pub(crate) struct RunResult {
    pub(crate) text: String,
    pub(crate) session_path: Option<PathBuf>,
}

struct SessionRecorder {
    id: String,
    dir: PathBuf,
    cwd: PathBuf,
    ready: bool,
}

pub(crate) fn model_router_config(config: &Config, args: &Args) -> ChatConfig {
    ChatConfig {
        url: env::var("MODEL_ROUTER_URL")
            .ok()
            .or(config.model_router_url.clone())
            .unwrap_or_else(|| "https://model-router-1080673333190.us-central1.run.app".into()),
        agent_id: env::var("WISENT_APP_AGENT_ID")
            .ok()
            .or(config.agent_id.clone())
            .unwrap_or_else(|| "wisent-app".into()),
        secret: env::var("WISENT_APP_AGENT_AUTH_SECRET").unwrap_or_default(),
        model: args
            .model
            .clone()
            .or(config.model.clone())
            .or_else(|| env::var("JEDEN_MODEL").ok())
            .unwrap_or_else(|| "claude-code-subscription".into()),
        service_tier: env::var("JEDEN_SERVICE_TIER")
            .ok()
            .or_else(|| env::var("MODEL_SERVICE_TIER").ok())
            .unwrap_or_default(),
    }
}

pub(crate) fn run_command(args: &Args) -> Result<String, String> {
    let task = args.positionals.join(" ").trim().to_string();
    if task.trim_start().starts_with('/') {
        let text = handle_slash(&args.cwd, task.trim(), args.model.as_deref())?;
        return Ok(if args.json { json!({ "text": text }).to_string() + "\n" } else { text + "\n" });
    }

    let result = run_no_tool_agent(args, &task)?;
    if args.json {
        return Ok(serde_json::to_string_pretty(&json!({
            "ok": true,
            "repaired": false,
            "originalError": Value::Null,
            "text": result.text,
            "sessionPath": result.session_path,
        }))
        .map_err(|e| e.to_string())?
            + "\n");
    }

    if let Some(path) = &result.session_path {
        eprintln!("[session] {}", path.display());
    }
    Ok(result.text + "\n")
}

fn run_no_tool_agent(args: &Args, task: &str) -> Result<RunResult, String> {
    let config = load_config(&args.cwd);
    let router = model_router_config(&config, args);
    let mut recorder = SessionRecorder::new(&args.cwd);
    recorder.ensure()?;
    recorder.record(
        "user",
        json!({
            "task": task,
            "cwd": args.cwd,
            "allowWrite": args.allow_write,
            "allowCommand": args.allow_command,
            "maxSteps": args.max_steps,
            "maxTokens": args.max_tokens,
        }),
    )?;

    let tool_specs = rust_tool_specs();
    let mut messages = vec![
        json!({ "role": "system", "content": system_prompt(&args.cwd) }),
        json!({ "role": "user", "content": task }),
    ];

    for step in 1..=args.max_steps {
        match chat_completion(&router, messages.clone(), args.max_tokens as usize, &tool_specs) {
            Ok(content) => {
                recorder.record("assistant_raw", json!({ "step": step, "content": content }))?;
                let action = action_or_text(&content)?;
                recorder.record("action", json!({ "step": step, "action": action_to_value(&action) }))?;
                messages.push(json!({ "role": "assistant", "content": content }));

                match action {
                    Action::Final { text } => {
                        recorder.record("final", json!({ "step": step, "text": text }))?;
                        return Ok(RunResult { text, session_path: Some(recorder.path()) });
                    }
                    Action::Tool { tool, input } => {
                        let result = run_tool_action(args, &mut recorder, step, &ToolAction { tool, input })?;
                        messages.push(json!({ "role": "user", "content": crate::tool_runtime::format_tool_result(&result) }));
                    }
                    Action::Tools { tools } => {
                        let mut results = Vec::new();
                        for tool in tools {
                            results.push(run_tool_action(args, &mut recorder, step, &tool)?);
                        }
                        messages.push(json!({ "role": "user", "content": crate::tool_runtime::format_tool_result(&json!(results)) }));
                    }
                }
            }
            Err(error) => {
                recorder.record("run_error", json!({ "message": error }))?;
                return Err(error);
            }
        }
    }

    let err = format!("max steps exceeded: {}", args.max_steps);
    recorder.record("run_error", json!({ "message": err }))?;
    Err(err)
}

fn action_or_text(content: &str) -> Result<Action, String> {
    match extract_json_object(content) {
        Ok(_) => parse_action(content),
        Err(error) if error.starts_with("model returned non-json content") => Ok(Action::Final { text: content.to_string() }),
        Err(error) => Err(error),
    }
}

fn action_to_value(action: &Action) -> Value {
    match action {
        Action::Final { text } => json!({ "action": "final", "text": text }),
        Action::Tool { tool, input } => json!({ "action": "tool", "tool": tool, "input": input }),
        Action::Tools { tools } => json!({ "action": "tools", "tools": tools.iter().map(tool_to_value).collect::<Vec<_>>() }),
    }
}

fn tool_to_value(action: &ToolAction) -> Value {
    json!({ "tool": action.tool, "input": action.input })
}

fn run_tool_action(args: &Args, recorder: &mut SessionRecorder, step: u32, action: &ToolAction) -> Result<Value, String> {
    recorder.record("tool_call", json!({ "step": step, "tool": action.tool, "input": action.input }))?;
    let runtime = crate::tool_runtime::ToolRuntime {
        cwd: &args.cwd,
        artifact_dir: Some(&recorder.artifact_dir()),
        allow_write: args.allow_write,
        allow_command: args.allow_command,
    };
    let result = match crate::tool_runtime::execute(&runtime, &action.tool, &action.input) {
        Ok(result) => result,
        Err(error) => json!({ "ok": false, "error": error }),
    };
    recorder.record("tool_result", json!({ "step": step, "tool": action.tool, "result": result }))?;
    Ok(result)
}

fn rust_tool_specs() -> Vec<Value> {
    vec![
        tool_spec("list_dir", "List a directory under cwd", json!({"path": {"type": "string"}, "limit": {"type": "number"}}), vec![]),
        tool_spec("read_file", "Read a UTF-8 file under cwd", json!({"path": {"type": "string"}}), vec!["path"]),
        tool_spec("read_binary_file", "Read one binary file under cwd as base64", json!({"path": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["path"]),
        tool_spec("read_image", "Read one PNG, JPEG, GIF, or WebP image under cwd as base64 with mime type and dimensions", json!({"path": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["path"]),
        tool_spec("read_sqlite", "Read a SQLite database under cwd: list tables, inspect a table, fetch one row, or run a read-only SELECT/WITH query", json!({"path": {"type": "string"}, "table": {"type": "string"}, "key": {"type": "string"}, "query": {"type": "string"}, "limit": {"type": "number"}, "offset": {"type": "number"}, "where": {"type": "string"}, "order": {"type": "string"}}), vec!["path"]),
        tool_spec("search_text", "Search one file for a literal string", json!({"path": {"type": "string"}, "query": {"type": "string"}, "caseSensitive": {"type": "boolean"}}), vec!["path", "query"]),
        tool_spec("search_files", "Recursively search text files under cwd for a literal string", json!({"path": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}}, "query": {"type": "string"}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "caseSensitive": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec!["query"]),
        tool_spec("glob_paths", "Find files under cwd with simple glob patterns", json!({"patterns": {"type": "string"}, "path": {"type": "string"}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec![]),
        tool_spec("grep_regex", "Search text files under cwd with a regular expression", json!({"expr": {"type": "string"}, "path": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "multiline": {"type": "boolean"}, "caseSensitive": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec!["expr"]),
        tool_spec("write_file", "Create or overwrite a UTF-8 file under cwd; overwrites require expectedSha256 and --allow-write", json!({"path": {"type": "string"}, "content": {"type": "string"}, "expectedSha256": {"type": "string"}}), vec!["path", "content"]),
        tool_spec("delete_file", "Delete one file under cwd; requires expectedSha256 and --allow-write", json!({"path": {"type": "string"}, "expectedSha256": {"type": "string"}}), vec!["path", "expectedSha256"]),
        tool_spec("move_file", "Move or rename one file under cwd; requires expectedSha256 and --allow-write", json!({"from": {"type": "string"}, "to": {"type": "string"}, "expectedSha256": {"type": "string"}, "overwrite": {"type": "boolean"}}), vec!["from", "to", "expectedSha256"]),
        tool_spec("run_command", "Run a shell command in cwd; requires --allow-command", json!({"command": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["command"]),
        tool_spec("run_process", "Run one process with argv array in cwd; requires --allow-command", json!({"command": {"type": "string"}, "args": {"type": "array", "items": {"type": "string"}}, "stdin": {"type": "string"}, "timeoutMs": {"type": "number"}, "env": {"type": "object"}}), vec!["command"]),
        tool_spec("node_eval", "Run JavaScript with node --input-type=module in cwd; requires --allow-command", json!({"code": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["code"]),
        tool_spec("python_eval", "Run Python code with python3 in cwd; requires --allow-command", json!({"code": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["code"]),
        tool_spec("list_package_scripts", "List package.json scripts in cwd", json!({}), vec![]),
        tool_spec("run_package_script", "Run one existing package.json script with npm; requires --allow-command", json!({"script": {"type": "string"}, "timeoutMs": {"type": "number"}, "env": {"type": "object"}}), vec!["script"]),
        tool_spec("git_status", "Read git status --short for cwd", json!({}), vec![]),
        tool_spec("git_diff", "Read git diff for cwd or one path under cwd", json!({"path": {"type": "string"}}), vec![]),
        tool_spec("git_log", "Read recent git commits for cwd or one path under cwd", json!({"limit": {"type": "number"}, "path": {"type": "string"}}), vec![]),
        tool_spec("git_show", "Read one git object or commit summary", json!({"ref": {"type": "string"}, "path": {"type": "string"}}), vec![]),
        tool_spec("fetch_url", "Fetch one HTTP(S) URL and return capped text; supports optional line range", json!({"url": {"type": "string"}, "maxBytes": {"type": "number"}, "timeoutMs": {"type": "number"}, "range": {"type": "string"}}), vec!["url"]),
        tool_spec("save_artifact", "Save UTF-8 content into the current session artifacts directory", json!({"name": {"type": "string"}, "content": {"type": "string"}}), vec!["content"]),
        tool_spec("list_artifacts", "List files in the current session artifact directory", json!({}), vec![]),
        tool_spec("read_artifact", "Read one UTF-8 artifact from the current session artifact directory", json!({"name": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["name"]),
        tool_spec("todo", "Manage the current session todo list with init, append, start, done, drop, rm, and view operations", json!({"op": {"type": "string"}, "list": {"type": "array"}, "phase": {"type": "string"}, "items": {"type": "array"}, "task": {"type": "string"}}), vec!["op"]),
        tool_spec("delegate_task", "Run a focused subtask in a fresh Jeden session and return its result; requires --allow-command", json!({"task": {"type": "string"}, "maxSteps": {"type": "number"}}), vec!["task"]),
        tool_spec("memory", "Remember and recall durable scoped notes across Jeden sessions", json!({"op": {"type": "string"}, "text": {"type": "string"}, "query": {"type": "string"}, "tags": {"type": "array"}, "limit": {"type": "number"}, "kind": {"type": "string"}, "scope": {"type": "object"}, "confidence": {"type": "number"}}), vec!["op"]),
        tool_spec("mcp_list_tools", "List tools from a configured stdio MCP server", json!({"server": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server"]),
        tool_spec("mcp_call_tool", "Call one tool on a configured stdio MCP server", json!({"server": {"type": "string"}, "tool": {"type": "string"}, "args": {"type": "object"}, "timeoutMs": {"type": "number"}}), vec!["server", "tool"]),
        tool_spec("mcp_list_resources", "List resources from a configured stdio MCP server", json!({"server": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server"]),
        tool_spec("mcp_read_resource", "Read one resource from a configured stdio MCP server", json!({"server": {"type": "string"}, "uri": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server", "uri"]),
        tool_spec("mcp_list_prompts", "List prompts from a configured stdio MCP server", json!({"server": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server"]),
        tool_spec("mcp_get_prompt", "Get one prompt from a configured stdio MCP server", json!({"server": {"type": "string"}, "name": {"type": "string"}, "args": {"type": "object"}, "timeoutMs": {"type": "number"}}), vec!["server", "name"]),
    ]
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

fn system_prompt(cwd: &Path) -> String {
    let executable = ["list_dir", "read_file", "read_binary_file", "read_image", "read_sqlite", "search_text", "search_files", "glob_paths", "grep_regex", "write_file", "delete_file", "move_file", "run_command", "run_process", "node_eval", "python_eval", "list_package_scripts", "run_package_script", "git_status", "git_diff", "git_log", "git_show", "fetch_url", "save_artifact", "list_artifacts", "read_artifact", "todo", "delegate_task", "memory", "mcp_list_tools", "mcp_call_tool", "mcp_list_resources", "mcp_read_resource", "mcp_list_prompts", "mcp_get_prompt"];
    let tools = crate::tools::list_tools(cwd)
        .into_iter()
        .filter(|tool| executable.contains(&tool.name.as_str()))
        .map(|tool| format!("- {}: {}", tool.name, tool.description))
        .collect::<Vec<_>>()
        .join("\n");
    format!("You are Jeden, Wisent's private agent harness.\n\nRules:\n- Answer with {{\"action\":\"final\",\"text\":\"your concise answer\"}} when done.\n- Use tool calls when the model-router supports native tool_calls, or answer with {{\"action\":\"tool\",\"tool\":\"tool_name\",\"input\":{{...}}}}.\n- Do not create tests unless the user explicitly asks.\n- Do not create docs unless the user explicitly asks.\n- Do not invent files, command outputs, or tool results.\n- Write tools require --allow-write; command tools require --allow-command.\n\nExecutable Rust tools:\n{}", tools)
}

impl SessionRecorder {
    fn new(cwd: &Path) -> Self {
        let id = stamp();
        Self { dir: session_root().join(&id), id, cwd: cwd.to_path_buf(), ready: false }
    }

    fn ensure(&mut self) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }
        fs::create_dir_all(self.dir.join("artifacts")).map_err(|e| e.to_string())?;
        let state_path = self.dir.join("state.json");
        if !state_path.exists() {
            let state = json!({ "id": self.id, "cwd": self.cwd, "startedAt": now_stamp() });
            fs::write(&state_path, serde_json::to_string_pretty(&state).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("transcript.jsonl"))
            .map_err(|e| e.to_string())?;
        self.ready = true;
        Ok(())
    }

    fn record(&mut self, event_type: &str, data: Value) -> Result<(), String> {
        self.ensure()?;
        let event = json!({ "ts": now_stamp(), "type": event_type, "data": data });
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("transcript.jsonl"))
            .map_err(|e| e.to_string())?;
        writeln!(file, "{}", event).map_err(|e| e.to_string())
    }

    fn artifact_dir(&self) -> PathBuf {
        self.dir.join("artifacts")
    }

    fn path(&self) -> PathBuf {
        self.dir.clone()
    }
}

fn stamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let suffix: String = rand::thread_rng().sample_iter(&Alphanumeric).take(6).map(char::from).collect();
    format!("{}-{}", secs, suffix)
}

fn now_stamp() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs().to_string()
}
