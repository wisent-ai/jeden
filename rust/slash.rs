use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tools;

#[derive(Debug, Clone)]
pub struct SlashContext<'a> {
    pub cwd: &'a Path,
    pub model: Option<&'a str>,
    pub session_root: &'a Path,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ModeState {
    #[serde(default)]
    plan: PlanState,
    #[serde(default)]
    goal: GoalState,
    #[serde(default)]
    loop_mode: LoopState,
    #[serde(default)]
    fast: FastState,
    #[serde(default)]
    advisor: AdvisorState,
    #[serde(default)]
    compact: bool,
    #[serde(default)]
    shake: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PlanState {
    #[serde(default)]
    enabled: bool,
    #[serde(rename = "latestPlan", default)]
    latest_plan: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct GoalState {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    objective: String,
    #[serde(default)]
    budget: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LoopState {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    remaining: Option<u64>,
    #[serde(default)]
    until: Option<u64>,
    #[serde(default)]
    prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FastState {
    #[serde(default)]
    enabled: bool,
    #[serde(rename = "serviceTier", default = "default_service_tier")]
    service_tier: String,
}

impl Default for FastState {
    fn default() -> Self { Self { enabled: false, service_tier: default_service_tier() } }
}

fn default_service_tier() -> String { "priority".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AdvisorState {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    model: String,
    #[serde(rename = "lastReview", default)]
    last_review: Option<Value>,
}

fn mode_state_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/mode-state.json") }

fn read_mode_state(cwd: &Path) -> ModeState {
    fs::read_to_string(mode_state_path(cwd))
        .ok()
        .and_then(|text| serde_json::from_str::<ModeState>(&text).ok())
        .unwrap_or_default()
}

fn write_mode_state(cwd: &Path, state: &ModeState) -> Result<(), String> {
    let path = mode_state_path(cwd);
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())? + "\n";
    fs::write(path, text).map_err(|e| e.to_string())
}

fn split_head(args: &str) -> (&str, &str) {
    let text = args.trim();
    if text.is_empty() { return ("", ""); }
    match text.find(char::is_whitespace) {
        Some(index) => (&text[..index], text[index..].trim()),
        None => (text, ""),
    }
}

fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn parse_duration_ms(value: &str) -> Option<u64> {
    let lower = value.trim().to_ascii_lowercase();
    let split = lower.find(|ch: char| !ch.is_ascii_digit())?;
    let amount = lower[..split].parse::<u64>().ok()?;
    match &lower[split..] {
        "ms" => Some(amount),
        "s" => Some(amount * 1_000),
        "m" => Some(amount * 60_000),
        "h" => Some(amount * 3_600_000),
        _ => None,
    }
}

fn format_goal_status(goal: &GoalState) -> String {
    if goal.objective.is_empty() { return "Goal mode has no objective. Use /goal set <objective>.".into(); }
    let state = if goal.enabled { if goal.paused { "paused" } else { "active" } } else { "disabled" };
    let budget = goal.budget.map(|v| if v.fract() == 0.0 { format!("{}", v as i64) } else { v.to_string() }).unwrap_or_else(|| "off".into());
    format!("Goal mode: {}\nObjective: {}\nBudget: {}", state, goal.objective, budget)
}

fn format_loop_status(loop_state: &LoopState) -> String {
    if !loop_state.enabled { return "Loop mode is disabled.".into(); }
    let mut limits = Vec::new();
    if let Some(remaining) = loop_state.remaining { limits.push(format!("{} resubmission(s) remaining", remaining)); }
    if let Some(until) = loop_state.until { limits.push(format!("until epoch-ms {}", until)); }
    if !loop_state.prompt.is_empty() { limits.push(format!("prompt: {}", loop_state.prompt)); }
    if limits.is_empty() { "Loop mode is enabled.".into() } else { format!("Loop mode is enabled ({}).", limits.join(", ")) }
}

fn current_model_route(context: &SlashContext<'_>) -> String {
    context.model
        .map(ToString::to_string)
        .or_else(|| env::var("JEDEN_MODEL").ok())
        .or_else(|| env::var("MODEL").ok())
        .unwrap_or_else(|| "default".into())
}

fn advisor_model_label(advisor: &AdvisorState, context: &SlashContext<'_>) -> String {
    if advisor.model.is_empty() { current_model_route(context) } else { advisor.model.clone() }
}

fn format_advisor_status(advisor: &AdvisorState, context: &SlashContext<'_>) -> String {
    [
        format!("Advisor reviewer is {}.", if advisor.enabled { "enabled" } else { "disabled" }),
        "Review backend: second model-router call after each successful agent result.".to_string(),
        format!("Configured reviewer route: {}.", advisor_model_label(advisor, context)),
        if advisor.last_review.is_some() { "Last advisor notes are available with /advisor dump.".to_string() } else { "No advisor notes have been recorded yet.".to_string() },
    ].join("\n")
}

fn handle_plan(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    if args.trim().is_empty() {
        state.plan.enabled = !state.plan.enabled;
        return Ok(format!("Plan mode {}.", if state.plan.enabled { "enabled" } else { "disabled" }));
    }
    match verb.as_str() {
        "on" => { state.plan.enabled = true; Ok("Plan mode enabled.".into()) },
        "off" => { state.plan.enabled = false; Ok("Plan mode disabled.".into()) },
        "status" => Ok(format!("Plan mode is {}.{}", if state.plan.enabled { "enabled" } else { "disabled" }, if state.plan.latest_plan.is_empty() { "" } else { "\nLatest plan is available for /plan-review." })),
        "run" if !rest.is_empty() => { state.plan.enabled = true; Ok("Plan mode enabled for this prompt.".into()) },
        _ => { state.plan.enabled = true; Ok("Plan mode enabled for this prompt.".into()) },
    }
}

fn handle_goal(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    if args.trim().is_empty() || verb == "show" || verb == "status" { return Ok(format_goal_status(&state.goal)); }
    match verb.as_str() {
        "set" => {
            if rest.is_empty() { return Err("Usage: /goal set <objective>".into()); }
            state.goal.objective = rest.to_string();
            state.goal.enabled = true;
            state.goal.paused = false;
            Ok(format!("Goal mode enabled.\nObjective: {}", state.goal.objective))
        },
        "pause" => { state.goal.paused = true; Ok("Goal mode paused.".into()) },
        "resume" => {
            if state.goal.objective.is_empty() { return Err("No goal objective is set. Use /goal set <objective>.".into()); }
            state.goal.enabled = true;
            state.goal.paused = false;
            Ok("Goal mode resumed.".into())
        },
        "drop" | "off" => {
            state.goal.enabled = false;
            state.goal.paused = false;
            state.goal.objective.clear();
            state.goal.budget = None;
            Ok("Goal mode dropped.".into())
        },
        "budget" => {
            let budget = rest.trim().to_ascii_lowercase();
            if budget.is_empty() || budget == "off" {
                state.goal.budget = None;
                return Ok("Goal budget disabled.".into());
            }
            let parsed = budget.parse::<f64>().map_err(|_| "Usage: /goal budget <positive-number|off>".to_string())?;
            if !parsed.is_finite() || parsed <= 0.0 { return Err("Usage: /goal budget <positive-number|off>".into()); }
            state.goal.budget = Some(parsed);
            Ok(format!("Goal budget set to {}.", if parsed.fract() == 0.0 { format!("{}", parsed as i64) } else { parsed.to_string() }))
        },
        _ => {
            state.goal.objective = args.trim().to_string();
            state.goal.enabled = true;
            state.goal.paused = false;
            Ok(format!("Goal mode enabled.\nObjective: {}", state.goal.objective))
        },
    }
}

fn handle_loop(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    if verb == "off" || verb == "stop" {
        state.loop_mode = LoopState::default();
        return Ok("Loop mode disabled.".into());
    }
    if verb == "status" { return Ok(format_loop_status(&state.loop_mode)); }
    let mut prompt = args.trim();
    state.loop_mode.remaining = None;
    state.loop_mode.until = None;
    if !head.is_empty() && head.chars().all(|ch| ch.is_ascii_digit()) {
        state.loop_mode.remaining = head.parse::<u64>().ok();
        prompt = rest;
    } else if let Some(duration) = parse_duration_ms(head) {
        state.loop_mode.until = Some(now_millis() + duration);
        prompt = rest;
    }
    state.loop_mode.enabled = true;
    state.loop_mode.prompt = prompt.to_string();
    let qualifier = if let Some(remaining) = state.loop_mode.remaining {
        format!(" for {} resubmission(s)", remaining)
    } else if state.loop_mode.until.is_some() {
        " until the duration expires".to_string()
    } else {
        String::new()
    };
    Ok(format!("Loop mode enabled{}.", qualifier))
}

fn handle_fast(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    match verb.as_str() {
        "" => state.fast.enabled = !state.fast.enabled,
        "on" => state.fast.enabled = true,
        "off" => state.fast.enabled = false,
        "tier" => {
            if rest.is_empty() { return Err("Usage: /fast tier <service-tier>".into()); }
            state.fast.service_tier = rest.to_string();
            state.fast.enabled = true;
        },
        "status" => {},
        _ => return Err("Usage: /fast [on|off|status|tier <service-tier>]".into()),
    }
    let tier = if state.fast.service_tier.is_empty() { "priority" } else { &state.fast.service_tier };
    Ok(format!("Fast mode is {}. Model-router service_tier for future requests: {}.", if state.fast.enabled { "enabled" } else { "disabled" }, if state.fast.enabled { tier } else { "(default)" }))
}

fn handle_advisor(args: &str, state: &mut ModeState, context: &SlashContext<'_>) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = if head.is_empty() { "status".to_string() } else { head.to_ascii_lowercase() };
    match verb.as_str() {
        "on" => {
            state.advisor.enabled = true;
            Ok(format!("Advisor reviewer enabled.\n{}", format_advisor_status(&state.advisor, context)))
        },
        "off" => {
            state.advisor.enabled = false;
            Ok("Advisor reviewer disabled.".into())
        },
        "status" => Ok(format_advisor_status(&state.advisor, context)),
        "dump" => {
            let Some(review) = &state.advisor.last_review else { return Err("No advisor notes are available yet. Enable /advisor and complete an agent turn first.".into()); };
            if rest.trim().eq_ignore_ascii_case("raw") { return serde_json::to_string_pretty(review).map_err(|e| e.to_string()); }
            Ok(review.get("text").and_then(Value::as_str).unwrap_or("Advisor review is empty.").to_string())
        },
        "configure" => {
            let config_text = rest.trim();
            if config_text.is_empty() { return Ok(format_advisor_status(&state.advisor, context)); }
            let (key, value_rest) = split_head(config_text);
            let mut model = config_text.to_string();
            if key.eq_ignore_ascii_case("model") { model = value_rest.trim().to_string(); }
            else if let Some((left, right)) = key.split_once('=') {
                if left.eq_ignore_ascii_case("model") { model = right.to_string(); }
            }
            if model.is_empty() { return Err("Usage: /advisor configure [model <route>|model=<route>|<route>]".into()); }
            state.advisor.model = model;
            Ok(format!("Advisor reviewer route set to {}.\n{}", state.advisor.model, format_advisor_status(&state.advisor, context)))
        },
        _ => Err("Usage: /advisor [on|off|status|dump [raw]|configure [model <route>|model=<route>|<route>]]".into()),
    }
}

fn list_sessions(session_root: &Path, limit: usize) -> String {
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(session_root) {
        for entry in entries.flatten().take(limit) { rows.push(entry.file_name().to_string_lossy().to_string()); }
    }
    if rows.is_empty() { "No sessions found.".into() } else { rows.join("\n") }
}

fn session_path(session_root: &Path, id_or_path: &str) -> PathBuf {
    if id_or_path.contains('/') { PathBuf::from(id_or_path) } else { session_root.join(id_or_path) }
}

fn handle_session(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, _) = split_head(args);
    if verb.is_empty() || verb == "info" {
        return Ok(format!("Session: rust one-shot slash invocation\nWorkspace: {}\nSession root: {}\nRecorder: not active in this non-interactive Rust command", context.cwd.display(), context.session_root.display()));
    }
    if verb == "delete" { return Err("Refusing to delete the active session from inside itself. Exit Jeden, then remove the session directory explicitly if you still want this destructive action.".into()); }
    Err("Usage: /session [info|delete]".into())
}

fn handle_lifecycle(command: &str, args: &str, state: &mut ModeState, context: &SlashContext<'_>) -> Option<Result<String, String>> {
    match command {
        "/new" | "/fresh" => Some(Ok("Started a fresh logical turn context. Provider stream state is reset for the next prompt in this Jeden process.".into())),
        "/drop" => Some(Err("Refusing to delete the active session from inside itself. Use /new for a fresh context or exit and remove the session directory explicitly.".into())),
        "/compact" => {
            state.compact = true;
            Some(Ok("Compact mode enabled for subsequent prompts: large prior context should be summarized before use.".into()))
        },
        "/shake" => {
            state.shake = if args.trim().is_empty() { "elide".into() } else { args.trim().into() };
            Some(Ok(format!("Shake mode applied locally: {}. Subsequent prompts will instruct the model to avoid relying on heavy prior artifacts unless re-read.", state.shake)))
        },
        "/resume" => {
            let (id, _) = split_head(args);
            if id.is_empty() { Some(Ok(list_sessions(context.session_root, 10))) }
            else {
                let path = session_path(context.session_root, id);
                if path.exists() { Some(Ok(format!("Session {} exists at {}. Full in-place interactive resume is available through CLI: jeden resume {} \"<task>\"", path.file_name().map(|v| v.to_string_lossy()).unwrap_or_default(), path.display(), path.display()))) }
                else { Some(Err(format!("session not found: {}", path.display()))) }
            }
        },
        "/rename" => Some(Ok(format!("Session title set to: {}", if args.trim().is_empty() { "rust one-shot slash invocation" } else { args.trim() }))),
        "/move" => Some(Err("/move requires an active interactive session recorder; Rust one-shot slash commands cannot move a live recorder in this pass.".into())),
        _ => None,
    }
}

fn handle_mcp(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    let verb = if verb.is_empty() { "list" } else { verb };
    match verb {
        "list" => {
            let names = tools::configured_mcp_server_names(context.cwd);
            Ok(if names.is_empty() { "No MCP servers configured.".into() } else { names.join("\n") })
        },
        "tools" | "resources" | "prompts" | "test" => {
            let (server, _) = split_head(rest);
            if server.is_empty() { return Err(format!("Usage: /mcp {} <server>", verb)); }
            Err(format!("MCP {} for {} requires a live stdio MCP client. Rust currently lists configured servers and static tool metadata without starting external MCP processes.", verb, server))
        },
        "reload" | "reconnect" => Ok("MCP clients closed; Rust has no persistent MCP clients in this one-shot command.".into()),
        _ => Err("Usage: /mcp list | tools <server> | resources <server> | prompts <server> | test <server> | reload | reconnect".into()),
    }
}

pub fn handle_local(context: &SlashContext<'_>, input: &str) -> Option<Result<String, String>> {
    let trimmed = input.trim();
    let (command, args) = split_head(trimmed);
    let command = command.to_ascii_lowercase();
    let mut state = read_mode_state(context.cwd);
    let mut changed = false;
    let result = match command.as_str() {
        "/plan" => { changed = args.trim() != "status"; Some(handle_plan(args, &mut state)) },
        "/goal" => { changed = !matches!(split_head(args).0, "" | "show" | "status"); Some(handle_goal(args, &mut state)) },
        "/loop" => { changed = split_head(args).0 != "status"; Some(handle_loop(args, &mut state)) },
        "/fast" => { changed = split_head(args).0 != "status"; Some(handle_fast(args, &mut state)) },
        "/advisor" => { changed = !matches!(split_head(args).0, "" | "status" | "dump"); Some(handle_advisor(args, &mut state, context)) },
        "/plan-review" => Some(if state.plan.latest_plan.is_empty() { Err("No plan is available to review yet. Run a prompt while /plan is enabled, then use /plan-review.".into()) } else { Ok("Reopening the latest plan for review.".into()) }),
        "/tools" => Some(Ok(tools::tools_slash_text(context.cwd))),
        "/session" => Some(handle_session(args, context)),
        "/mcp" => Some(handle_mcp(args, context)),
        "/new" | "/fresh" | "/drop" | "/compact" | "/shake" | "/resume" | "/rename" | "/move" => {
            changed = matches!(command.as_str(), "/compact" | "/shake");
            handle_lifecycle(command.as_str(), args, &mut state, context)
        },
        "/agents" => Some(Ok("Agent controls:\n- /tan <work> starts a detached local agent job tracked in session artifacts.\n- /advisor manages second-pass reviewer mode.\n- /jobs shows locally tracked background jobs.".into())),
        "/jobs" => Some(Ok("No background jobs are tracked inside this Jeden process.".into())),
        "/changelog" => Some(Ok("No bundled changelog is present in Jeden. Git history is the source of release notes for this package.".into())),
        "/hotkeys" => Some(Ok("Jeden interactive hotkeys:\nEnter submits the prompt.\nCtrl-J inserts a newline.\nLeft/Right/Home/End edit inside the prompt.\nUp/Down navigate prompt history.\nCtrl-C exits input mode or denies approval.".into())),
        _ => None,
    };
    if changed {
        if let Some(Ok(_)) = &result {
            if let Err(error) = write_mode_state(context.cwd, &state) { return Some(Err(error)); }
        }
    }
    result
}
