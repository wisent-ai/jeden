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

/// Cooperative controls for a turn: cancellation polled between steps, a
/// progress sink for live TUI status, and an interactive flag that gates
/// stdin-reading tools when the turn runs on a background thread.
pub(crate) struct RunHooks<'a> {
    pub(crate) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) interactive: bool,
    pub(crate) progress: Box<dyn Fn(&str) + 'a>,
}

impl RunHooks<'static> {
    /// Non-interactive-safe default for the CLI `run` path (no TUI, no cancel).
    pub(crate) fn inert() -> Self {
        Self {
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interactive: true,
            progress: Box::new(|_| {}),
        }
    }
}

impl RunHooks<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn note(&self, message: &str) {
        (self.progress)(message);
    }
}

struct SessionRecorder {
    id: String,
    dir: PathBuf,
    cwd: PathBuf,
    ready: bool,
}

pub(crate) fn model_router_config(config: &Config, args: &Args) -> ChatConfig {
    let mode_state = read_mode_state(&args.cwd);
    let mode_service_tier = if mode_state.pointer("/fast/enabled").and_then(Value::as_bool).unwrap_or(false) {
        mode_state.pointer("/fast/serviceTier").and_then(Value::as_str).filter(|value| !value.trim().is_empty()).map(str::to_string)
    } else {
        None
    };
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
            .or(mode_service_tier)
            .unwrap_or_default(),
    }
}

fn mode_state_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/mode-state.json")
}

fn read_mode_state(cwd: &Path) -> Value {
    fs::read_to_string(mode_state_path(cwd))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn write_mode_state(cwd: &Path, state: &Value) -> Result<(), String> {
    let path = mode_state_path(cwd);
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    fs::write(path, serde_json::to_string_pretty(state).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())
}

fn apply_mode_instructions(cwd: &Path, task: &str) -> Result<String, String> {
    let mut state = read_mode_state(cwd);
    let Some(map) = state.as_object_mut() else { return Ok(task.to_string()); };
    let mut parts = Vec::new();
    if let Some(force) = map.get("force").and_then(Value::as_object) {
        if let Some(tool) = force.get("tool").and_then(Value::as_str).filter(|tool| !tool.is_empty()) {
            parts.push(format!("Forced tool request for this turn: use tool \"{}\" first if it is applicable and available. If it is unsafe or inapplicable, explain why before using another tool.", tool));
            map.insert("force".into(), Value::Null);
            write_mode_state(cwd, &state)?;
        }
    }
    if parts.is_empty() { Ok(task.to_string()) } else { Ok(format!("{}\n\n{}", parts.join("\n"), task)) }
}

pub(crate) fn update_task_outcome(cwd: &Path, task: &str, ok: bool) -> Result<(), String> {
    let mut state = read_mode_state(cwd);
    if !state.is_object() { state = json!({}); }
    let map = state.as_object_mut().expect("mode state object");
    if ok {
        map.insert("lastTask".into(), json!(task));
        map.insert("lastFailedTask".into(), json!(""));
    } else {
        map.insert("lastFailedTask".into(), json!(task));
    }
    write_mode_state(cwd, &state)
}

pub(crate) fn update_last_session_path(cwd: &Path, path: &Path) -> Result<(), String> {
    let mut state = read_mode_state(cwd);
    if !state.is_object() { state = json!({}); }
    state.as_object_mut().expect("mode state object").insert("lastSessionPath".into(), json!(path));
    write_mode_state(cwd, &state)
}

fn split_head(args: &str) -> (&str, &str) {
    let text = args.trim();
    if text.is_empty() { return ("", ""); }
    match text.find(char::is_whitespace) {
        Some(index) => (&text[..index], text[index..].trim()),
        None => (text, ""),
    }
}

/// Resolve the prompt text a `/retry` should replay, or an error. Lets the
/// interactive loop replay through the shared Conversation instead of a
/// transient one.
pub(crate) fn retry_task(args: &Args) -> Result<String, String> {
    let state = read_mode_state(&args.cwd);
    let task = state
        .get("lastFailedTask")
        .and_then(Value::as_str)
        .filter(|task| !task.trim().is_empty())
        .ok_or("No failed task is available to retry.")?;
    if task.trim_start().starts_with('/') {
        return Err("Refusing to retry a slash command; retry only replays agent prompts.".into());
    }
    Ok(task.to_string())
}

/// Build the scoped prompt text for a `/btw` side question.
pub(crate) fn btw_task(question: &str) -> Result<String, String> {
    let question = question.trim();
    if question.is_empty() { return Err("Usage: /btw <side question>".into()); }
    Ok(format!(
        "Answer this side question using the current session context.\nKeep it separate from the main task: do not change files unless the side question explicitly asks for file changes.\nQuestion: {}",
        question
    ))
}

pub(crate) fn retry_command_with(args: &Args, hooks: &mut RunHooks) -> Result<String, String> {
    let state = read_mode_state(&args.cwd);
    let task = state
        .get("lastFailedTask")
        .and_then(Value::as_str)
        .filter(|task| !task.trim().is_empty())
        .ok_or("No failed task is available to retry.")?;
    if task.trim_start().starts_with('/') {
        return Err("Refusing to retry a slash command; retry only replays agent prompts.".into());
    }
    let mut retry_args = args.clone();
    retry_args.command = "run".into();
    retry_args.positionals = vec![task.to_string()];
    run_command_with(&retry_args, hooks)
}

pub(crate) fn btw_command_with(args: &Args, question: &str, hooks: &mut RunHooks) -> Result<String, String> {
    let question = question.trim();
    if question.is_empty() { return Err("Usage: /btw <side question>".into()); }
    let mut side_args = args.clone();
    side_args.command = "run".into();
    side_args.positionals = vec![format!(
        "Answer this side question using the current session context.\nKeep it separate from the main task: do not change files unless the side question explicitly asks for file changes.\nQuestion: {}",
        question
    )];
    run_command_with(&side_args, hooks)
}

pub(crate) fn run_command(args: &Args) -> Result<String, String> {
    run_command_with(args, &mut RunHooks::inert())
}

pub(crate) fn run_command_with(args: &Args, hooks: &mut RunHooks) -> Result<String, String> {
    let task = args.positionals.join(" ").trim().to_string();
    if task.trim_start().starts_with('/') {
        let (command, rest) = split_head(task.trim());
        if command == "/retry" { return retry_command_with(args, hooks); }
        if command == "/btw" { return btw_command_with(args, rest, hooks); }
        // Unknown slash commands fall through to the model as a prompt (OMP
        // parity) instead of hard-erroring; builtins are handled locally.
        if crate::is_builtin_slash(command) {
            let text = handle_slash(&args.cwd, task.trim(), args.model.as_deref())?;
            return Ok(if args.json { json!({ "text": text }).to_string() + "\n" } else { text + "\n" });
        }
    }

    let mut conversation = Conversation::new(&args.cwd)?;
    let result = conversation.run_turn(args, &task, hooks);
    if let Err(error) = &result {
        let _ = update_task_outcome(&args.cwd, &task, false);
        return Err(error.clone());
    }
    let text = result?;
    let _ = update_task_outcome(&args.cwd, &task, true);
    let session_path = Some(conversation.session_path());
    let result = RunResult { text, session_path: session_path.clone() };
    if let Some(path) = &session_path {
        let _ = update_last_session_path(&args.cwd, path);
    }
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

/// A persistent agent conversation. In interactive mode one `Conversation`
/// lives for the whole session so each turn sees the full prior history (real
/// chat memory); the CLI one-shot builds a transient one per invocation.
pub(crate) struct Conversation {
    messages: Vec<Value>,
    recorder: SessionRecorder,
}

impl Conversation {
    pub(crate) fn new(cwd: &Path) -> Result<Self, String> {
        let mut recorder = SessionRecorder::new(cwd);
        recorder.ensure()?;
        Ok(Self {
            messages: vec![json!({ "role": "system", "content": system_prompt(cwd) })],
            recorder,
        })
    }

    pub(crate) fn session_path(&self) -> PathBuf {
        self.recorder.path()
    }

    /// Rough token estimate (~4 chars/token) over the live message window, for
    /// the status line. Not billing-accurate; a live signal, not a guess.
    pub(crate) fn approx_tokens(&self) -> usize {
        let chars: usize = self.messages.iter().map(|m| m.to_string().chars().count()).sum();
        chars / 4
    }

    /// Number of non-system messages currently held.
    pub(crate) fn turn_len(&self) -> usize {
        self.messages.iter().filter(|m| m.get("role").and_then(Value::as_str) != Some("system")).count()
    }

    /// Drop all turns, keeping the system prompt — backs /clear and /new.
    pub(crate) fn reset(&mut self, cwd: &Path) -> Result<(), String> {
        self.messages = vec![json!({ "role": "system", "content": system_prompt(cwd) })];
        self.recorder = SessionRecorder::new(cwd);
        self.recorder.ensure()
    }

    /// Replace the live history with prior user/assistant turns — backs /resume
    /// so a resumed session actually continues in-process.
    pub(crate) fn load_history(&mut self, cwd: &Path, turns: Vec<Value>) -> Result<(), String> {
        let mut messages = vec![json!({ "role": "system", "content": system_prompt(cwd) })];
        messages.extend(turns);
        self.messages = messages;
        Ok(())
    }

    /// Summarize the live history into a single compact system note and drop the
    /// detailed turns — backs a real /compact instead of a mode-state flag.
    pub(crate) fn compact(&mut self, args: &Args, instructions: &str, hooks: &mut RunHooks) -> Result<String, String> {
        if self.turn_len() == 0 {
            return Err("Nothing to compact yet; the conversation is empty.".into());
        }
        if hooks.cancelled() {
            return Err("Turn cancelled.".into());
        }
        hooks.note("compacting conversation");
        let config = load_config(&args.cwd);
        let router = model_router_config(&config, args);
        let transcript = self
            .messages
            .iter()
            .skip(1)
            .map(|m| {
                let role = m.get("role").and_then(Value::as_str).unwrap_or("?");
                let content = m.get("content").and_then(Value::as_str).unwrap_or("");
                format!("{}: {}", role, content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let extra = if instructions.trim().is_empty() { String::new() } else { format!("\nFocus the summary on: {}", instructions.trim()) };
        let ask = vec![
            json!({ "role": "system", "content": "You compress a coding-agent conversation into a durable brief. Reply with plain text only." }),
            json!({ "role": "user", "content": format!("Summarize the conversation below so work can continue with full context but far fewer tokens. Preserve decisions, file paths, open tasks, and constraints.{}\n\n---\n{}", extra, transcript) }),
        ];
        let summary = chat_completion(&router, ask, args.max_tokens as usize, &[])?;
        if hooks.cancelled() {
            return Err("Turn cancelled.".into());
        }
        let before = self.turn_len();
        self.recorder.record("compaction", json!({ "before": before, "summary": summary }))?;
        self.messages = vec![
            json!({ "role": "system", "content": system_prompt(&args.cwd) }),
            json!({ "role": "system", "content": format!("Prior conversation summary (compacted from {} messages):\n{}", before, summary) }),
        ];
        Ok(format!("Compacted {} messages into a summary.\n\n{}", before, summary))
    }

    pub(crate) fn run_turn(&mut self, args: &Args, task: &str, hooks: &mut RunHooks) -> Result<String, String> {
        let config = load_config(&args.cwd);
        let router = model_router_config(&config, args);
        let effective_task = apply_mode_instructions(&args.cwd, task)?;
        self.recorder.record(
            "user",
            json!({
                "task": effective_task,
                "cwd": args.cwd,
                "allowWrite": args.allow_write,
                "allowCommand": args.allow_command,
                "maxSteps": args.max_steps,
                "maxTokens": args.max_tokens,
            }),
        )?;

        let tool_specs = rust_tool_specs();
        self.messages.push(json!({ "role": "user", "content": effective_task }));

        for step in 1..=args.max_steps {
            if hooks.cancelled() {
                let err = "Turn cancelled.".to_string();
                self.recorder.record("run_error", json!({ "message": err }))?;
                return Err(err);
            }
            hooks.note(&format!("thinking (step {}/{})", step, args.max_steps));
            match chat_completion(&router, self.messages.clone(), args.max_tokens as usize, &tool_specs) {
                Ok(content) => {
                    self.recorder.record("assistant_raw", json!({ "step": step, "content": content }))?;
                    let action = action_or_text(&content)?;
                    self.recorder.record("action", json!({ "step": step, "action": action_to_value(&action) }))?;
                    self.messages.push(json!({ "role": "assistant", "content": content }));

                    match action {
                        Action::Final { text } => {
                            self.recorder.record("final", json!({ "step": step, "text": text }))?;
                            // Persist the user-visible answer (not the raw JSON
                            // action blob) so the next turn's context is clean.
                            if let Some(last) = self.messages.last_mut() {
                                last["content"] = json!(text);
                            }
                            return Ok(text);
                        }
                        Action::Tool { tool, input } => {
                            if hooks.cancelled() {
                                let err = "Turn cancelled.".to_string();
                                self.recorder.record("run_error", json!({ "message": err }))?;
                                return Err(err);
                            }
                            hooks.note(&format!("tool: {}", tool));
                            let result = run_tool_action(args, &mut self.recorder, step, &ToolAction { tool, input }, hooks.interactive)?;
                            self.messages.push(json!({ "role": "user", "content": crate::tool_runtime::format_tool_result(&result) }));
                        }
                        Action::Tools { tools } => {
                            let mut results = Vec::new();
                            for tool in tools {
                                if hooks.cancelled() {
                                    let err = "Turn cancelled.".to_string();
                                    self.recorder.record("run_error", json!({ "message": err }))?;
                                    return Err(err);
                                }
                                hooks.note(&format!("tool: {}", tool.tool));
                                results.push(run_tool_action(args, &mut self.recorder, step, &tool, hooks.interactive)?);
                            }
                            self.messages.push(json!({ "role": "user", "content": crate::tool_runtime::format_tool_result(&json!(results)) }));
                        }
                    }
                }
                Err(error) => {
                    self.recorder.record("run_error", json!({ "message": error }))?;
                    return Err(error);
                }
            }
        }

        let err = format!("max steps exceeded: {}", args.max_steps);
        self.recorder.record("run_error", json!({ "message": err }))?;
        Err(err)
    }
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

fn run_tool_action(args: &Args, recorder: &mut SessionRecorder, step: u32, action: &ToolAction, interactive: bool) -> Result<Value, String> {
    recorder.record("tool_call", json!({ "step": step, "tool": action.tool, "input": action.input }))?;
    let runtime = crate::tool_runtime::ToolRuntime {
        cwd: &args.cwd,
        artifact_dir: Some(&recorder.artifact_dir()),
        allow_write: args.allow_write,
        allow_command: args.allow_command,
        interactive,
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
        tool_spec("read_document", "Extract readable text from one document under cwd with optional line range", json!({"path": {"type": "string"}, "maxBytes": {"type": "number"}, "range": {"type": "string"}}), vec!["path"]),
        tool_spec("read_archive", "List archive entries or read one entry from .zip, .tar, .tar.gz, or .tgz under cwd", json!({"path": {"type": "string"}, "entry": {"type": "string"}, "mode": {"type": "string"}, "maxBytes": {"type": "number"}, "range": {"type": "string"}}), vec!["path"]),
        tool_spec("read_image", "Read one PNG, JPEG, GIF, or WebP image under cwd as base64 with mime type and dimensions", json!({"path": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["path"]),
        tool_spec("read_sqlite", "Read a SQLite database under cwd: list tables, inspect a table, fetch one row, or run a read-only SELECT/WITH query", json!({"path": {"type": "string"}, "table": {"type": "string"}, "key": {"type": "string"}, "query": {"type": "string"}, "limit": {"type": "number"}, "offset": {"type": "number"}, "where": {"type": "string"}, "order": {"type": "string"}}), vec!["path"]),
        tool_spec("search_text", "Search one file for a literal string", json!({"path": {"type": "string"}, "query": {"type": "string"}, "caseSensitive": {"type": "boolean"}}), vec!["path", "query"]),
        tool_spec("search_files", "Recursively search text files under cwd for a literal string", json!({"path": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}}, "query": {"type": "string"}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "caseSensitive": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec!["query"]),
        tool_spec("glob_paths", "Find files under cwd with simple glob patterns", json!({"patterns": {"type": "string"}, "path": {"type": "string"}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec![]),
        tool_spec("grep_regex", "Search text files under cwd with a regular expression", json!({"expr": {"type": "string"}, "path": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "multiline": {"type": "boolean"}, "caseSensitive": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec!["expr"]),
        tool_spec("write_file", "Create or overwrite a UTF-8 file under cwd; overwrites require expectedSha256 and --allow-write", json!({"path": {"type": "string"}, "content": {"type": "string"}, "expectedSha256": {"type": "string"}}), vec!["path", "content"]),
        tool_spec("apply_patch", "Apply exact one-occurrence string replacements to an existing UTF-8 file; requires expectedSha256 and --allow-write", json!({"path": {"type": "string"}, "expectedSha256": {"type": "string"}, "replacements": {"type": "array"}}), vec!["path", "expectedSha256", "replacements"]),
        tool_spec("edit_file", "Apply line-based edits to a UTF-8 file under cwd; requires expectedSha256 and --allow-write", json!({"path": {"type": "string"}, "expectedSha256": {"type": "string"}, "ops": {"type": "array"}}), vec!["path", "expectedSha256", "ops"]),
        tool_spec("edit", "Apply an OMP-style anchored visual patch string with [path#TAG], SWAP/DEL/INS/REM/MV and safe block hunks; requires --allow-write", json!({"patch": {"type": "string"}}), vec!["patch"]),
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
        tool_spec("fetch_readable_url", "Fetch one HTTP(S) URL and return simplified readable text with optional line range", json!({"url": {"type": "string"}, "maxBytes": {"type": "number"}, "timeoutMs": {"type": "number"}, "range": {"type": "string"}}), vec!["url"]),
        tool_spec("save_artifact", "Save UTF-8 content into the current session artifacts directory", json!({"name": {"type": "string"}, "content": {"type": "string"}}), vec!["content"]),
        tool_spec("list_artifacts", "List files in the current session artifact directory", json!({}), vec![]),
        tool_spec("read_artifact", "Read one UTF-8 artifact from the current session artifact directory", json!({"name": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["name"]),
        tool_spec("ask_user", "Ask the human user a question during an interactive session", json!({"question": {"type": "string"}, "options": {"type": "array"}}), vec!["question"]),
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
    let executable = ["list_dir", "read_file", "read_binary_file", "read_document", "read_archive", "read_image", "read_sqlite", "search_text", "search_files", "glob_paths", "grep_regex", "write_file", "apply_patch", "edit_file", "edit", "delete_file", "move_file", "run_command", "run_process", "node_eval", "python_eval", "list_package_scripts", "run_package_script", "git_status", "git_diff", "git_log", "git_show", "fetch_url", "fetch_readable_url", "save_artifact", "list_artifacts", "read_artifact", "todo", "delegate_task", "memory", "ask_user", "mcp_list_tools", "mcp_call_tool", "mcp_list_resources", "mcp_read_resource", "mcp_list_prompts", "mcp_get_prompt"];
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Mutex, OnceLock};

    // Env vars are process-global; serialize every test that mutates
    // JEDEN_SESSION_ROOT so a temp session root set by one test can never leak
    // into a concurrently running one.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = env::var_os(key);
            env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(previous) => env::set_var(self.key, previous),
                None => env::remove_var(self.key),
            }
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("jeden-agent-{}-{}-{}-{}", name, std::process::id(), nanos, seq));
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// A `Conversation` writes a session dir on construction; point it at a
    /// throwaway root so tests never touch ~/.jeden. Returns the guard so the
    /// caller keeps the env var alive for the whole test.
    fn temp_session_root(name: &str) -> EnvVarGuard {
        EnvVarGuard::set("JEDEN_SESSION_ROOT", unique_temp_dir(name))
    }

    fn args_with_cwd(cwd: &Path) -> Args {
        Args {
            command: "interactive".into(),
            cwd: cwd.to_path_buf(),
            model: None,
            max_tokens: 2048,
            max_steps: 8,
            allow_write: false,
            allow_command: false,
            json: false,
            positionals: vec![],
        }
    }

    fn user_turn(text: &str) -> Value {
        json!({ "role": "user", "content": text })
    }

    fn assistant_turn(text: &str) -> Value {
        json!({ "role": "assistant", "content": text })
    }

    #[test]
    fn new_conversation_holds_no_turns_but_counts_the_system_prompt() {
        let _env = env_lock().lock().unwrap();
        let _root = temp_session_root("new");
        let cwd = unique_temp_dir("new-cwd");

        let conv = Conversation::new(&cwd).expect("conversation constructs under a temp session root");

        // A fresh conversation carries only the system message: zero turns...
        assert_eq!(conv.turn_len(), 0);
        // ...but the system prompt itself is real text, so the token estimate
        // is strictly positive (the system message is counted).
        assert!(conv.approx_tokens() > 0, "system prompt should contribute tokens, got {}", conv.approx_tokens());
    }

    #[test]
    fn load_history_installs_the_given_turns_and_grows_the_token_estimate() {
        let _env = env_lock().lock().unwrap();
        let _root = temp_session_root("load");
        let cwd = unique_temp_dir("load-cwd");

        let mut conv = Conversation::new(&cwd).expect("conversation constructs");
        let empty_tokens = conv.approx_tokens();
        assert_eq!(conv.turn_len(), 0, "sanity: starts empty");

        conv.load_history(
            &cwd,
            vec![
                user_turn("Refactor the parser to stream tokens instead of buffering the whole file."),
                assistant_turn("Done — parser now streams; see rust/protocol.rs for the new incremental reader."),
            ],
        )
        .expect("load_history succeeds");

        // Two non-system turns were loaded (system prompt is not a turn).
        assert_eq!(conv.turn_len(), 2);
        // Adding real turns strictly increases the character count, so the
        // (chars/4) estimate must exceed the empty-conversation estimate.
        assert!(
            conv.approx_tokens() > empty_tokens,
            "loading turns should grow tokens: {} !> {}",
            conv.approx_tokens(),
            empty_tokens
        );
    }

    #[test]
    fn reset_after_load_history_drops_back_to_the_system_prompt() {
        let _env = env_lock().lock().unwrap();
        let _root = temp_session_root("reset");
        let cwd = unique_temp_dir("reset-cwd");

        let mut conv = Conversation::new(&cwd).expect("conversation constructs");
        conv.load_history(&cwd, vec![user_turn("first task"), assistant_turn("first answer")])
            .expect("load_history succeeds");
        assert_eq!(conv.turn_len(), 2, "precondition: history is loaded");

        conv.reset(&cwd).expect("reset succeeds");

        // /clear semantics: every turn is gone, only the system prompt remains.
        assert_eq!(conv.turn_len(), 0);
        assert!(conv.approx_tokens() > 0, "system prompt survives reset");
    }

    #[test]
    fn retry_task_errors_when_no_failed_task_is_recorded() {
        // No mode-state file at all: nothing to retry.
        let cwd = unique_temp_dir("retry-missing");
        let err = retry_task(&args_with_cwd(&cwd)).expect_err("missing mode-state has no task to retry");
        assert!(err.contains("No failed task"), "unexpected error: {err}");

        // Mode-state exists but lastFailedTask is blank: still nothing to retry.
        let cwd = unique_temp_dir("retry-blank");
        write_mode_state(&cwd, &json!({ "lastFailedTask": "   " })).unwrap();
        let err = retry_task(&args_with_cwd(&cwd)).expect_err("blank lastFailedTask is not retryable");
        assert!(err.contains("No failed task"), "unexpected error: {err}");
    }

    #[test]
    fn retry_task_returns_the_recorded_failed_task() {
        let cwd = unique_temp_dir("retry-set");
        write_mode_state(&cwd, &json!({ "lastFailedTask": "rebuild the search index" })).unwrap();

        let task = retry_task(&args_with_cwd(&cwd)).expect("a recorded failed task is replayable");
        assert_eq!(task, "rebuild the search index");
    }

    #[test]
    fn retry_task_refuses_to_replay_a_slash_command() {
        let cwd = unique_temp_dir("retry-slash");
        write_mode_state(&cwd, &json!({ "lastFailedTask": "/compact focus on the parser" })).unwrap();

        let err = retry_task(&args_with_cwd(&cwd)).expect_err("slash commands must not be retried as prompts");
        assert!(err.contains("slash command"), "unexpected error: {err}");
    }

    #[test]
    fn btw_task_rejects_empty_or_blank_questions() {
        assert!(btw_task("").is_err(), "empty question is a usage error");
        assert!(btw_task("   ").is_err(), "whitespace-only question is a usage error");
    }

    #[test]
    fn btw_task_scopes_the_prompt_and_carries_the_question() {
        let prompt = btw_task("why did the last build fail").expect("a real question yields a prompt");
        // The caller's question must survive verbatim into the scoped prompt...
        assert!(prompt.contains("why did the last build fail"), "question missing from prompt: {prompt}");
        // ...framed as a side question that must not touch files.
        assert!(prompt.contains("side question"), "prompt is not scoped as a side question: {prompt}");
    }
}
