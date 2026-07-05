use rand::{distributions::Alphanumeric, Rng};
use serde_json::{json, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model_router::{chat_completion, chat_completion_streaming, ChatConfig};
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
    /// Per-token streaming sink for live assistant text.
    pub(crate) stream: Box<dyn Fn(&str) + 'a>,
    /// Ask the user to approve a gated tool that isn't pre-authorized. Returns
    /// true to allow this one call. Inert default denies (CLI = flags only).
    pub(crate) approve: Box<dyn Fn(&str) -> bool + 'a>,
}

impl RunHooks<'static> {
    /// Non-interactive-safe default for the CLI `run` path (no TUI, no cancel).
    pub(crate) fn inert() -> Self {
        Self {
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interactive: true,
            progress: Box::new(|_| {}),
            stream: Box::new(|_| {}),
            approve: Box::new(|_| false),
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

    fn push_delta(&self, piece: &str) {
        (self.stream)(piece);
    }

    fn approve(&self, tool: &str) -> bool {
        (self.approve)(tool)
    }
}

/// Tools that mutate the filesystem (require write authorization).
pub(crate) fn is_write_tool(tool: &str) -> bool {
    matches!(tool, "write_file" | "apply_patch" | "edit_file" | "edit" | "delete_file" | "move_file")
}

/// Tools that execute commands/code (require command authorization).
pub(crate) fn is_command_tool(tool: &str) -> bool {
    matches!(tool, "run_command" | "run_process" | "node_eval" | "python_eval" | "run_package_script" | "delegate_task")
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
    let state = read_mode_state(cwd);
    let mut parts = Vec::new();
    // /force: one-shot forced tool for the next turn, then cleared.
    if let Some(tool) = state.pointer("/force/tool").and_then(Value::as_str).filter(|tool| !tool.is_empty()) {
        parts.push(format!("Forced tool request for this turn: use tool \"{}\" first if it is applicable and available. If it is unsafe or inapplicable, explain why before using another tool.", tool));
        let mut cleared = state.clone();
        if let Some(map) = cleared.as_object_mut() {
            map.insert("force".into(), Value::Null);
            write_mode_state(cwd, &cleared)?;
        }
    }
    // /plan: research + plan, no file changes.
    if state.pointer("/plan/enabled").and_then(Value::as_bool).unwrap_or(false) {
        parts.push("Plan mode is active: research and lay out a concrete, ordered plan for this task before doing the work. Do not modify files unless the user explicitly asks in this turn; end with the plan.".to_string());
    }
    // /goal: keep every step aligned with the stored objective.
    if state.pointer("/goal/enabled").and_then(Value::as_bool).unwrap_or(false) {
        if let Some(objective) = state.pointer("/goal/objective").and_then(Value::as_str).filter(|o| !o.trim().is_empty()) {
            let budget = state.pointer("/goal/budget").and_then(Value::as_f64).map(|b| format!(" Respect the working budget of {}.", b)).unwrap_or_default();
            parts.push(format!("Active goal: {}. Keep every step aligned with this goal and note progress toward it.{}", objective.trim(), budget));
        }
    }
    // /shake: distrust heavy prior context unless re-read.
    if let Some(shake) = state.get("shake").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        parts.push(format!("Shake mode ({}): do not rely on heavy prior context or artifacts unless you re-read them this turn; re-verify assumptions from source.", shake.trim()));
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
    let mut task = args.positionals.join(" ").trim().to_string();
    if task.trim_start().starts_with('/') {
        let (command, rest) = split_head(task.trim());
        if command == "/retry" { return retry_command_with(args, hooks); }
        if command == "/btw" { return btw_command_with(args, rest, hooks); }
        // Builtins handled locally; file-based custom commands expand to a
        // prompt; anything else falls through to the model literally (OMP parity).
        if crate::is_builtin_slash(command) {
            let text = handle_slash(&args.cwd, task.trim(), args.model.as_deref())?;
            return Ok(if args.json { json!({ "text": text }).to_string() + "\n" } else { text + "\n" });
        }
        if let Some(expanded) = crate::resolve_file_command(&args.cwd, command, rest) {
            task = expanded;
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

    /// Fork: keep the current in-memory history but switch to a NEW session dir
    /// so subsequent turns record into a separate lineage — backs /fork as a
    /// real session split, not a mode-state label. Returns the new session path.
    pub(crate) fn fork(&mut self, cwd: &Path) -> Result<PathBuf, String> {
        let parent = self.recorder.path();
        self.recorder = SessionRecorder::new(cwd);
        self.recorder.ensure()?;
        self.recorder.record("fork", json!({ "parent": parent }))?;
        Ok(self.recorder.path())
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

    /// Generate an LLM handoff brief from the live history, write it to the
    /// session artifacts, then start a fresh session seeded with the brief —
    /// a real /handoff (OMP generateHandoff parity), not a raw transcript dump.
    pub(crate) fn handoff(&mut self, args: &Args, focus: &str, hooks: &mut RunHooks) -> Result<String, String> {
        if self.turn_len() == 0 {
            return Err("Nothing to hand off yet; the conversation is empty.".into());
        }
        if hooks.cancelled() {
            return Err("Turn cancelled.".into());
        }
        hooks.note("generating handoff");
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
        let extra = if focus.trim().is_empty() { String::new() } else { format!("\nFocus the handoff on: {}", focus.trim()) };
        let ask = vec![
            json!({ "role": "system", "content": "You write a handoff brief so a fresh agent session can continue this work with no prior context. Reply with a concise plain-text brief: goal, decisions, files touched, open tasks, next steps." }),
            json!({ "role": "user", "content": format!("Write the handoff brief for the conversation below.{}\n\n---\n{}", extra, transcript) }),
        ];
        let brief = chat_completion(&router, ask, args.max_tokens as usize, &[])?;
        if hooks.cancelled() {
            return Err("Turn cancelled.".into());
        }
        let artifact_dir = self.recorder.artifact_dir();
        fs::create_dir_all(&artifact_dir).map_err(|e| e.to_string())?;
        let file = artifact_dir.join("handoff.md");
        let doc = if focus.trim().is_empty() { brief.clone() } else { format!("Focus: {}\n\n{}", focus.trim(), brief) };
        fs::write(&file, &doc).map_err(|e| e.to_string())?;
        self.recorder.record("handoff", json!({ "focus": focus, "brief": brief, "file": file }))?;
        // Start a fresh session seeded with the handoff brief.
        self.recorder = SessionRecorder::new(&args.cwd);
        self.recorder.ensure()?;
        self.messages = vec![
            json!({ "role": "system", "content": system_prompt(&args.cwd) }),
            json!({ "role": "system", "content": format!("Handoff brief from the prior session:\n{}", brief) }),
        ];
        Ok(format!("Handoff brief written to {} and a fresh session was started seeded with it.\n\n{}", file.display(), brief))
    }

    /// If /advisor is enabled, run a second reviewer pass over the answer and
    /// append its critique. Best-effort: a reviewer failure never fails the turn.
    fn maybe_advisor_review(&mut self, args: &Args, answer: String, hooks: &mut RunHooks) -> Result<String, String> {
        let state = read_mode_state(&args.cwd);
        if !state.pointer("/advisor/enabled").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(answer);
        }
        if hooks.cancelled() {
            return Ok(answer);
        }
        hooks.note("advisor review");
        let config = load_config(&args.cwd);
        let mut router = model_router_config(&config, args);
        if let Some(model) = state.pointer("/advisor/model").and_then(Value::as_str).filter(|m| !m.trim().is_empty()) {
            router.model = model.to_string();
        }
        let ask = vec![
            json!({ "role": "system", "content": "You are a second-pass reviewer. Critique the assistant's answer for correctness, gaps, and risks in 2-4 concise bullet points. If it is sound, say so briefly. Reply with plain text only." }),
            json!({ "role": "user", "content": format!("Assistant answer to review:\n\n{}", answer) }),
        ];
        match chat_completion(&router, ask, args.max_tokens as usize, &[]) {
            Ok(review) => {
                self.recorder.record("advisor", json!({ "review": review })).ok();
                Ok(format!("{}\n\n— Advisor review —\n{}", answer, review))
            }
            Err(error) => Ok(format!("{}\n\n(advisor review unavailable: {})", answer, error)),
        }
    }

    pub(crate) fn run_turn(&mut self, args: &Args, task: &str, hooks: &mut RunHooks) -> Result<String, String> {
        let config = load_config(&args.cwd);
        let router = model_router_config(&config, args);
        let mut effective_task = apply_mode_instructions(&args.cwd, task)?;
        let hook_context = crate::hooks::user_prompt_submit(&args.cwd, task, args.allow_command);
        if !hook_context.trim().is_empty() {
            effective_task = format!("{}\n\n[Hook context]\n{}", effective_task, hook_context.trim());
        }
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
            // Stream deltas, but suppress anything that looks like a raw JSON
            // action/tool blob so its syntax never leaks to the UI. Buffer until
            // the first non-whitespace character decides plain-text vs JSON.
            let decided = std::cell::Cell::new(false);
            let suppress = std::cell::Cell::new(false);
            let pending = std::cell::RefCell::new(String::new());
            let mut on_delta = |piece: &str| {
                if !decided.get() {
                    pending.borrow_mut().push_str(piece);
                    let buf = pending.borrow().clone();
                    let lead = buf.trim_start();
                    if lead.is_empty() {
                        return; // only whitespace so far; keep buffering
                    }
                    decided.set(true);
                    suppress.set(lead.starts_with('{') || lead.starts_with('['));
                    if !suppress.get() {
                        hooks.push_delta(&buf);
                    }
                    pending.borrow_mut().clear();
                    return;
                }
                if !suppress.get() {
                    hooks.push_delta(piece);
                }
            };
            let call = chat_completion_streaming(&router, self.messages.clone(), args.max_tokens as usize, &tool_specs, &mut on_delta);
            match call {
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
                            return self.maybe_advisor_review(args, text, hooks);
                        }
                        Action::Tool { tool, input } => {
                            if hooks.cancelled() {
                                let err = "Turn cancelled.".to_string();
                                self.recorder.record("run_error", json!({ "message": err }))?;
                                return Err(err);
                            }
                            let (aw, ac) = effective_allows(args, &tool, hooks);
                            let result = if let Some(reason) = crate::hooks::pretool_block(&args.cwd, &tool, &input, args.allow_command) {
                                hooks.note(&format!("tool blocked by hook: {}", tool));
                                json!({ "ok": false, "error": format!("blocked by PreToolUse hook: {}", reason) })
                            } else {
                                if tool_will_run(&tool, aw, ac) {
                                    hooks.note(&format!("tool: {}", tool));
                                } else {
                                    hooks.note(&format!("tool denied: {}", tool));
                                }
                                let r = run_tool_action(args, &mut self.recorder, step, &ToolAction { tool: tool.clone(), input: input.clone() }, hooks.interactive, aw, ac)?;
                                crate::hooks::posttool(&args.cwd, &tool, &r, args.allow_command);
                                r
                            };
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
                                let (aw, ac) = effective_allows(args, &tool.tool, hooks);
                                if let Some(reason) = crate::hooks::pretool_block(&args.cwd, &tool.tool, &tool.input, args.allow_command) {
                                    hooks.note(&format!("tool blocked by hook: {}", tool.tool));
                                    results.push(json!({ "ok": false, "error": format!("blocked by PreToolUse hook: {}", reason) }));
                                    continue;
                                }
                                if tool_will_run(&tool.tool, aw, ac) {
                                    hooks.note(&format!("tool: {}", tool.tool));
                                } else {
                                    hooks.note(&format!("tool denied: {}", tool.tool));
                                }
                                let r = run_tool_action(args, &mut self.recorder, step, &tool, hooks.interactive, aw, ac)?;
                                crate::hooks::posttool(&args.cwd, &tool.tool, &r, args.allow_command);
                                results.push(r);
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

/// Effective (allow_write, allow_command) for one tool call. Pre-authorized by
/// CLI flags, else the user is asked to approve a gated tool once via the hook.
fn effective_allows(args: &Args, tool: &str, hooks: &RunHooks) -> (bool, bool) {
    let mut allow_write = args.allow_write;
    let mut allow_command = args.allow_command;
    if is_write_tool(tool) && !allow_write && hooks.approve(tool) {
        allow_write = true;
    }
    if is_command_tool(tool) && !allow_command && hooks.approve(tool) {
        allow_command = true;
    }
    (allow_write, allow_command)
}

/// Whether a tool will actually execute given the effective authorization. A
/// gated tool denied approval will error inside the runtime, so it never runs.
fn tool_will_run(tool: &str, allow_write: bool, allow_command: bool) -> bool {
    if is_write_tool(tool) {
        return allow_write;
    }
    if is_command_tool(tool) {
        return allow_command;
    }
    true
}

fn run_tool_action(args: &Args, recorder: &mut SessionRecorder, step: u32, action: &ToolAction, interactive: bool, allow_write: bool, allow_command: bool) -> Result<Value, String> {
    recorder.record("tool_call", json!({ "step": step, "tool": action.tool, "input": action.input }))?;
    let runtime = crate::tool_runtime::ToolRuntime {
        cwd: &args.cwd,
        artifact_dir: Some(&recorder.artifact_dir()),
        allow_write,
        allow_command,
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

    // ---- apply_mode_instructions -------------------------------------------
    // Per-turn directive injection wired to /plan, /goal, /shake, /force. Each
    // test owns a private temp cwd with its own .jeden/mode-state.json, so they
    // are order-independent, touch no env vars, and never reach ~/.jeden.

    #[test]
    fn apply_mode_instructions_returns_task_unchanged_when_no_state_file() {
        // A cwd with no .jeden/mode-state.json: nothing to inject, so the task
        // must pass through byte-for-byte.
        let cwd = unique_temp_dir("mode-none");
        let task = "summarize the changelog";

        let out = apply_mode_instructions(&cwd, task).expect("a missing state file is not an error");

        assert_eq!(out, task, "no mode-state must leave the task exactly as given");
    }

    #[test]
    fn apply_mode_instructions_prepends_plan_directive_and_keeps_task_last() {
        let cwd = unique_temp_dir("mode-plan");
        write_mode_state(&cwd, &json!({ "plan": { "enabled": true } })).unwrap();

        let task = "add streaming to the parser";
        let out = apply_mode_instructions(&cwd, task).expect("plan mode applies");

        // The plan directive is injected ahead of the task...
        assert!(out.contains("Plan mode is active"), "plan directive missing: {out}");
        // ...and the task itself stays verbatim at the tail.
        assert!(out.ends_with(task), "task must remain, unchanged, at the end: {out}");
        assert_ne!(out, task, "plan mode must actually prepend a directive, not pass through");
    }

    #[test]
    fn apply_mode_instructions_does_not_inject_plan_directive_when_disabled() {
        // plan present but enabled=false: the flag, not mere presence, gates it.
        let cwd = unique_temp_dir("mode-plan-off");
        write_mode_state(&cwd, &json!({ "plan": { "enabled": false } })).unwrap();

        let task = "add streaming to the parser";
        let out = apply_mode_instructions(&cwd, task).expect("disabled plan mode applies");

        assert!(!out.contains("Plan mode is active"), "disabled plan must inject nothing: {out}");
        assert_eq!(out, task, "no active mode means the task is returned unchanged");
    }

    #[test]
    fn apply_mode_instructions_injects_goal_objective_and_budget_note() {
        let cwd = unique_temp_dir("mode-goal");
        write_mode_state(
            &cwd,
            &json!({ "goal": { "enabled": true, "objective": "ship the parser", "budget": 5.0 } }),
        )
        .unwrap();

        let task = "keep working on the milestone";
        let out = apply_mode_instructions(&cwd, task).expect("goal mode applies");

        assert!(out.contains("Active goal:"), "goal label missing: {out}");
        assert!(out.contains("ship the parser"), "objective must be carried into the directive: {out}");
        // budget 5.0 renders via Display as `5`; the note must carry the value.
        assert!(out.contains("working budget of 5"), "budget note missing or wrong value: {out}");
        assert!(out.ends_with(task), "task must remain at the tail: {out}");
    }

    #[test]
    fn apply_mode_instructions_omits_budget_note_when_budget_unset() {
        // Objective without a budget: the goal directive fires, but there is no
        // budget clause to append.
        let cwd = unique_temp_dir("mode-goal-nobudget");
        write_mode_state(
            &cwd,
            &json!({ "goal": { "enabled": true, "objective": "ship the parser" } }),
        )
        .unwrap();

        let out = apply_mode_instructions(&cwd, "keep working").expect("goal mode applies");

        assert!(out.contains("Active goal:"), "goal label missing: {out}");
        assert!(out.contains("ship the parser"), "objective missing: {out}");
        assert!(!out.contains("working budget"), "no budget was set, so no budget note: {out}");
    }

    #[test]
    fn apply_mode_instructions_injects_shake_directive_with_its_value() {
        let cwd = unique_temp_dir("mode-shake");
        write_mode_state(&cwd, &json!({ "shake": "elide" })).unwrap();

        let task = "re-derive the token counts";
        let out = apply_mode_instructions(&cwd, task).expect("shake mode applies");

        assert!(out.contains("Shake mode"), "shake directive missing: {out}");
        // the concrete shake value labels the directive so it is specific.
        assert!(out.contains("elide"), "shake value missing from directive: {out}");
        assert!(out.ends_with(task), "task must remain at the tail: {out}");
    }

    #[test]
    fn apply_mode_instructions_injects_forced_tool_then_clears_it_one_shot() {
        let cwd = unique_temp_dir("mode-force");
        write_mode_state(&cwd, &json!({ "force": { "tool": "read_file" } })).unwrap();

        let task = "inspect the config";
        let out = apply_mode_instructions(&cwd, task).expect("force mode applies");

        // The forced-tool directive is injected for this turn, naming the tool.
        assert!(out.contains("Forced tool request"), "force directive missing: {out}");
        assert!(out.contains("read_file"), "forced tool name missing: {out}");

        // It is one-shot: the on-disk `force` is cleared to null so the next
        // turn is not forced again. Re-read the file and assert.
        let after = read_mode_state(&cwd);
        assert_eq!(
            after.get("force"),
            Some(&Value::Null),
            "force must be cleared to null after use, got: {after}"
        );

        // Proof of the clear's effect: a second call sees no force and, with no
        // other mode active, returns the task unchanged.
        let out2 = apply_mode_instructions(&cwd, task).expect("second call succeeds");
        assert!(!out2.contains("Forced tool request"), "force must not re-fire after being cleared: {out2}");
        assert_eq!(out2, task, "with force cleared and no other mode, task is returned unchanged: {out2}");
    }

    // ---- tiered approval: pure classification & authorization --------------
    // is_write_tool / is_command_tool / tool_will_run / effective_allows are
    // pure over their string+flag inputs (no fs, no threads, no PTY), so these
    // tests need no temp dirs, no env lock, and no session root.

    /// Build an `Args` with just the two authorization flags toggled; the cwd
    /// is never touched by the pure functions under test.
    fn args_with_flags(allow_write: bool, allow_command: bool) -> Args {
        let mut args = args_with_cwd(Path::new("/nonexistent"));
        args.allow_write = allow_write;
        args.allow_command = allow_command;
        args
    }

    /// A `RunHooks` whose only meaningful field is `approve`; everything else
    /// mirrors `RunHooks::inert()`. Lets a test inject an approve closure that
    /// records or decides, without spinning up the interactive machinery.
    fn hooks_with_approve<'a>(approve: impl Fn(&str) -> bool + 'a) -> RunHooks<'a> {
        RunHooks {
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interactive: true,
            progress: Box::new(|_| {}),
            stream: Box::new(|_| {}),
            approve: Box::new(approve),
        }
    }

    #[test]
    fn is_write_tool_matches_exactly_the_filesystem_mutators() {
        // Every tool that mutates the filesystem must be classified as a write
        // tool, or it would run without --allow-write / approval.
        for tool in ["write_file", "apply_patch", "edit_file", "edit", "delete_file", "move_file"] {
            assert!(is_write_tool(tool), "{tool} mutates the filesystem and must be a write tool");
        }
        // Read-only and command tools must NOT be gated as writes, or a harmless
        // read would demand write authorization.
        for tool in ["read_file", "list_dir", "run_command"] {
            assert!(!is_write_tool(tool), "{tool} does not write files and must not be a write tool");
        }
    }

    #[test]
    fn is_command_tool_matches_exactly_the_executors() {
        // Every tool that executes commands/code must be classified as a command
        // tool, or it would run without --allow-command / approval.
        for tool in ["run_command", "run_process", "node_eval", "python_eval", "run_package_script", "delegate_task"] {
            assert!(is_command_tool(tool), "{tool} executes code and must be a command tool");
        }
        // A read and a write tool are not executors: they must not trip the
        // command gate.
        for tool in ["read_file", "write_file"] {
            assert!(!is_command_tool(tool), "{tool} does not execute code and must not be a command tool");
        }
    }

    #[test]
    fn tool_will_run_gates_write_tools_on_allow_write() {
        // A write tool runs only when write authorization is present; the
        // command flag is irrelevant to it.
        assert!(!tool_will_run("write_file", false, true), "write tool must be blocked without allow_write");
        assert!(tool_will_run("write_file", true, false), "write tool must run with allow_write");
    }

    #[test]
    fn tool_will_run_gates_command_tools_on_allow_command() {
        // A command tool runs only when command authorization is present; the
        // write flag is irrelevant to it.
        assert!(!tool_will_run("run_command", true, false), "command tool must be blocked without allow_command");
        assert!(tool_will_run("run_command", false, true), "command tool must run with allow_command");
    }

    #[test]
    fn tool_will_run_lets_non_gated_tools_run_regardless_of_flags() {
        // A non-gated tool (read_file) is never authorization-gated: it must run
        // even with both flags denied. This is the branch a mutation from
        // `true` to `allow_write` would break.
        assert!(tool_will_run("read_file", false, false), "non-gated tool must run even with both flags false");
    }

    #[test]
    fn effective_allows_honors_preauthorized_write_without_consulting_approve() {
        // Pre-authorized by the CLI flag: the write flag is already true, so the
        // hook must never be asked. The panicking closure fails the test if it is.
        let args = args_with_flags(true, false);
        let hooks = hooks_with_approve(|_| panic!("approve must not be consulted when the tool is pre-authorized"));

        let (allow_write, allow_command) = effective_allows(&args, "write_file", &hooks);

        assert!(allow_write, "a pre-authorized write flag must remain granted");
        assert!(!allow_command, "the unrelated command flag must stay as the args left it");
    }

    #[test]
    fn effective_allows_elevates_write_when_approve_grants_it() {
        // Not pre-authorized, but the user approves: the write flag must be
        // elevated, and approve is consulted exactly once (only the write gate).
        let calls = std::cell::Cell::new(0usize);
        let args = args_with_flags(false, false);
        let hooks = hooks_with_approve(|_| {
            calls.set(calls.get() + 1);
            true
        });

        let (allow_write, allow_command) = effective_allows(&args, "write_file", &hooks);

        assert!(allow_write, "an approved write tool must be elevated to allowed");
        assert!(!allow_command, "approving a write tool must not touch the command flag");
        assert_eq!(calls.get(), 1, "only the write gate should consult approve, exactly once");
    }

    #[test]
    fn effective_allows_elevates_command_when_approve_grants_it() {
        // Symmetric to the write case: an approved command tool elevates only the
        // command flag, consulting approve exactly once.
        let calls = std::cell::Cell::new(0usize);
        let args = args_with_flags(false, false);
        let hooks = hooks_with_approve(|_| {
            calls.set(calls.get() + 1);
            true
        });

        let (allow_write, allow_command) = effective_allows(&args, "run_command", &hooks);

        assert!(allow_command, "an approved command tool must be elevated to allowed");
        assert!(!allow_write, "approving a command tool must not touch the write flag");
        assert_eq!(calls.get(), 1, "only the command gate should consult approve, exactly once");
    }

    #[test]
    fn effective_allows_stays_denied_when_approve_refuses() {
        // The user declines the one-shot approval: neither flag may be elevated.
        let args = args_with_flags(false, false);
        let hooks = hooks_with_approve(|_| false);

        let allows = effective_allows(&args, "write_file", &hooks);

        assert_eq!(allows, (false, false), "a refused write tool must stay fully denied");
    }

    #[test]
    fn effective_allows_never_consults_approve_for_non_gated_tools() {
        // A non-gated tool (read_file) is neither write nor command, so the hook
        // must never be asked and the flags pass through unchanged.
        let calls = std::cell::Cell::new(0usize);
        let args = args_with_flags(false, false);
        let hooks = hooks_with_approve(|_| {
            calls.set(calls.get() + 1);
            true
        });

        let allows = effective_allows(&args, "read_file", &hooks);

        assert_eq!(allows, (false, false), "a non-gated tool leaves both flags untouched");
        assert_eq!(calls.get(), 0, "a non-gated tool must never consult approve");
    }
}
