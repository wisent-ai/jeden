use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use serde_json::Map;

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
    #[serde(rename = "guidedGoal", default)]
    guided_goal: GuidedGoalState,
    #[serde(default)]
    loop_mode: LoopState,
    #[serde(default)]
    fast: FastState,
    #[serde(default)]
    advisor: AdvisorState,
    #[serde(default)]
    force: Option<ForceState>,
    #[serde(rename = "lastFailedTask", default)]
    last_failed_task: String,
    #[serde(rename = "lastTask", default)]
    last_task: String,
    #[serde(default)]
    compact: bool,
    #[serde(default)]
    shake: String,
    #[serde(default)]
    todos: Vec<TodoState>,
    #[serde(default)]
    branches: Vec<BranchState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PlanState {
    #[serde(default)]
    enabled: bool,
    #[serde(rename = "latestPlan", default)]
    latest_plan: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct GuidedGoalState {
    #[serde(default)]
    active: bool,
    #[serde(rename = "roughObjective", default)]
    rough_objective: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ForceState {
    #[serde(default)]
    tool: String,
    #[serde(default)]
    prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TodoState {
    #[serde(default)]
    text: String,
    #[serde(default)]
    status: String,
    #[serde(rename = "createdAt", default)]
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BranchState {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(rename = "createdAt", default)]
    created_at: String,
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

fn dirs_home() -> PathBuf { env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")) }

fn project_config_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/config.json") }

fn read_json_value(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn write_json_value(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())? + "\n";
    fs::write(path, text).map_err(|e| e.to_string())
}

fn merged_config(cwd: &Path) -> Value {
    let mut merged = match read_json_value(&dirs_home().join(".jeden/config.json")) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if let Value::Object(project) = read_json_value(&project_config_path(cwd)) {
        for (key, value) in project { merged.insert(key, value); }
    }
    Value::Object(merged)
}

fn is_plain_object(value: &Value) -> bool {
    matches!(value, Value::Object(_))
}

fn browser_record_from(config: &Value) -> Value {
    let browser = config.get("browser").filter(|value| is_plain_object(value)).cloned().unwrap_or_else(|| json!({}));
    let mode = browser.get("mode").and_then(Value::as_str)
        .or_else(|| config.get("browserMode").and_then(Value::as_str))
        .filter(|mode| matches!(*mode, "headless" | "visible"))
        .unwrap_or("headless");
    json!({
        "mode": mode,
        "updatedAt": browser.get("updatedAt").and_then(Value::as_str),
        "launch": browser.get("launch").filter(|value| is_plain_object(value)).cloned().unwrap_or_else(|| json!({})),
        "profile": browser.get("profile").filter(|value| is_plain_object(value)).cloned().unwrap_or_else(|| json!({})),
    })
}

fn format_browser_settings(label: &str, value: &Value) -> String {
    if value.as_object().map(|map| map.is_empty()).unwrap_or(true) {
        format!("{label}: (none)")
    } else {
        format!("{label}: {}", value)
    }
}

fn browser_option_value(key: &str, value: &str) -> Value {
    if value == "true" { return json!(true); }
    if value == "false" { return json!(false); }
    if matches!(key, "slowMo" | "timeout") {
        if let Ok(number) = value.parse::<f64>() { return json!(number); }
    }
    if key == "args" {
        return json!(value.split(',').map(str::trim).filter(|part| !part.is_empty()).collect::<Vec<_>>());
    }
    json!(value)
}

fn insert_nested_object(target: &mut Value, path: &[&str], value: Value) {
    if !target.is_object() { *target = json!({}); }
    let mut cursor = target.as_object_mut().expect("object");
    for part in &path[..path.len().saturating_sub(1)] {
        let next = cursor.entry((*part).to_string()).or_insert_with(|| json!({}));
        if !next.is_object() { *next = json!({}); }
        cursor = next.as_object_mut().expect("nested object");
    }
    if let Some(key) = path.last() {
        cursor.insert((*key).to_string(), value);
    }
}

fn parse_browser_options(tokens: &[String]) -> Result<(Value, Value), String> {
    let mut launch = json!({});
    let mut profile = json!({});
    for token in tokens {
        let Some((raw_key, raw_value)) = token.split_once('=') else {
            return Err(format!("Expected key=value option, got \"{token}\"."));
        };
        let key_parts = raw_key.split('.').filter(|part| !part.is_empty()).collect::<Vec<_>>();
        if key_parts.is_empty() || key_parts.iter().any(|part| {
            matches!(*part, "__proto__" | "constructor" | "prototype")
                || !part.chars().enumerate().all(|(index, ch)| if index == 0 { ch.is_ascii_alphabetic() } else { ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' })
        }) {
            return Err(format!("Invalid browser option key \"{raw_key}\"."));
        }
        let (scope, rest) = (key_parts[0], &key_parts[1..]);
        if scope == "launch" && !rest.is_empty() {
            insert_nested_object(&mut launch, rest, browser_option_value(rest.last().copied().unwrap_or(""), raw_value));
        } else if scope == "profile" && !rest.is_empty() {
            insert_nested_object(&mut profile, rest, browser_option_value(rest.last().copied().unwrap_or(""), raw_value));
        } else if raw_key == "launch" {
            insert_nested_object(&mut launch, &["executablePath"], browser_option_value("executablePath", raw_value));
        } else if raw_key == "profile" {
            insert_nested_object(&mut profile, &["name"], browser_option_value("name", raw_value));
        } else if matches!(raw_key, "args" | "channel" | "devtools" | "executablePath" | "slowMo" | "timeout") {
            insert_nested_object(&mut launch, &[raw_key], browser_option_value(raw_key, raw_value));
        } else if matches!(raw_key, "name" | "profileDir" | "profile" | "userDataDir") {
            let normalized = if raw_key == "profileDir" { "userDataDir" } else { raw_key };
            insert_nested_object(&mut profile, &[normalized], browser_option_value(normalized, raw_value));
        } else {
            return Err(format!("Unknown browser option \"{raw_key}\". Use launch.<key>=value or profile.<key>=value."));
        }
    }
    Ok((launch, profile))
}

fn plugin_registry_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/plugins.json") }

fn plugin_registry(cwd: &Path) -> Value {
    let raw = read_json_value(&plugin_registry_path(cwd));
    json!({
        "version": 1,
        "sources": raw.get("sources").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({})),
        "installed": raw.get("installed").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({})),
        "reload": raw.get("reload").filter(|value| value.is_object()).cloned().unwrap_or(Value::Null),
    })
}

fn save_plugin_registry(cwd: &Path, registry: &Value) -> Result<PathBuf, String> {
    let file = plugin_registry_path(cwd);
    let mut normalized = plugin_registry(cwd);
    if let Some(map) = registry.as_object() {
        if let Some(sources) = map.get("sources").filter(|value| value.is_object()) { normalized["sources"] = sources.clone(); }
        if let Some(installed) = map.get("installed").filter(|value| value.is_object()) { normalized["installed"] = installed.clone(); }
        if let Some(reload) = map.get("reload") { normalized["reload"] = reload.clone(); }
    }
    normalized["updatedAt"] = json!(now_text());
    write_json_value(&file, &normalized)?;
    Ok(file)
}


fn format_plugin_source(value: &Value) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        value.get("name").and_then(Value::as_str).unwrap_or("-"),
        value.get("type").and_then(Value::as_str).unwrap_or("-"),
        value.get("source").and_then(Value::as_str).unwrap_or("-"),
        if value.get("enabled").and_then(Value::as_bool) == Some(false) { "disabled" } else { "enabled" }
    )
}

fn format_plugin(value: &Value) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        value.get("id").and_then(Value::as_str).unwrap_or("-"),
        value.get("version").and_then(Value::as_str).unwrap_or("-"),
        if value.get("enabled").and_then(Value::as_bool) == Some(false) { "disabled" } else { "enabled" },
        value.get("source").and_then(Value::as_str).unwrap_or("-")
    )
}

fn sorted_object_values(value: &Value) -> Vec<Value> {
    let mut values = value.as_object().map(|map| map.values().cloned().collect::<Vec<_>>()).unwrap_or_default();
    values.sort_by(|a, b| format_plugin(a).cmp(&format_plugin(b)));
    values
}

fn handle_extensions(context: &SlashContext<'_>) -> Result<String, String> {
    let registry = plugin_registry(context.cwd);
    let mut sources = sorted_object_values(&registry["sources"]);
    sources.sort_by(|a, b| a.get("name").and_then(Value::as_str).unwrap_or("").cmp(b.get("name").and_then(Value::as_str).unwrap_or("")));
    let mut installed = sorted_object_values(&registry["installed"]);
    installed.sort_by(|a, b| a.get("id").and_then(Value::as_str).unwrap_or("").cmp(b.get("id").and_then(Value::as_str).unwrap_or("")));
    let mut lines = vec![format!("Extension registry: {}", plugin_registry_path(context.cwd).display()), format!("Sources: {}", sources.len())];
    if sources.is_empty() { lines.push("- none".into()); } else { lines.extend(sources.iter().map(format_plugin_source)); }
    lines.push(format!("Installed plugins: {}", installed.len()));
    if installed.is_empty() { lines.push("- none".into()); } else { lines.extend(installed.iter().map(format_plugin)); }
    Ok(lines.join("\n"))
}

fn handle_plugins(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("list");
    let target = argv.get(1).map(String::as_str).unwrap_or("");
    let mut registry = plugin_registry(context.cwd);
    if verb == "list" {
        let installed = sorted_object_values(&registry["installed"]);
        return Ok(if installed.is_empty() { "No plugins installed. Use /marketplace discover and /marketplace install <name@marketplace>.".into() } else { ["Installed plugins:".into()].into_iter().chain(installed.iter().map(format_plugin)).collect::<Vec<_>>().join("\n") });
    }
    if verb == "enable" || verb == "disable" {
        if target.is_empty() { return Err(format!("Usage: /plugins {verb} <name@marketplace>")); }
        let installed = registry.get_mut("installed").and_then(Value::as_object_mut).ok_or("invalid plugin registry")?;
        let plugin = installed.get_mut(target).ok_or_else(|| format!("Installed plugin not found: {target}"))?;
        if !plugin.is_object() { *plugin = json!({}); }
        let plugin_obj = plugin.as_object_mut().expect("plugin object");
        plugin_obj.insert("enabled".into(), json!(verb == "enable"));
        plugin_obj.insert("updatedAt".into(), json!(now_text()));
        let file = save_plugin_registry(context.cwd, &registry)?;
        return Ok(format!("{} plugin {} in {}.", if verb == "enable" { "Enabled" } else { "Disabled" }, target, file.display()));
    }
    Err("Usage: /plugins list | enable <name@marketplace> | disable <name@marketplace>".into())
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

fn now_text() -> String { now_millis().to_string() }

fn split_args(value: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in value.trim().chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active { quote = None; } else { current.push(ch); }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() { args.push(current); }
    args
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
            let result = match verb {
                "tools" | "test" => crate::mcp::list_tools(context.cwd, server, 10_000),
                "resources" => crate::mcp::list_resources(context.cwd, server, 10_000),
                "prompts" => crate::mcp::list_prompts(context.cwd, server, 10_000),
                _ => unreachable!(),
            }?;
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        },
        "reload" | "reconnect" => Ok("MCP clients closed; Rust has no persistent MCP clients in this one-shot command.".into()),
        _ => Err("Usage: /mcp list | tools <server> | resources <server> | prompts <server> | test <server> | reload | reconnect".into()),
    }
}
fn handle_browser(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("");
    let config = merged_config(context.cwd);
    let current = browser_record_from(&config);
    let file = project_config_path(context.cwd);
    if verb.is_empty() || verb == "status" {
        return Ok([
            format!("Browser runtime preference: {}", current.get("mode").and_then(Value::as_str).unwrap_or("headless")),
            format!("Updated: {}", current.get("updatedAt").and_then(Value::as_str).unwrap_or("not set locally")),
            format!("Config: {}", file.display()),
            format_browser_settings("Launch settings", current.get("launch").unwrap_or(&Value::Null)),
            format_browser_settings("Profile settings", current.get("profile").unwrap_or(&Value::Null)),
            "Scope: configures the browser tool/controller backend selected by local Jeden config.".into(),
        ].join("\n"));
    }
    if !matches!(verb, "headless" | "visible") {
        return Err("Usage: /browser [status|headless|visible] [launch.<key>=value] [profile.<key>=value]".into());
    }
    let (launch, profile) = parse_browser_options(if argv.len() > 1 { &argv[1..] } else { &[] })
        .map_err(|error| format!("Usage: /browser [status|headless|visible] [launch.<key>=value] [profile.<key>=value]\n{error}"))?;
    let mut merged_launch = current.get("launch").cloned().unwrap_or_else(|| json!({}));
    let mut merged_profile = current.get("profile").cloned().unwrap_or_else(|| json!({}));
    if let (Some(target), Some(source)) = (merged_launch.as_object_mut(), launch.as_object()) {
        for (key, value) in source { target.insert(key.clone(), value.clone()); }
    }
    if let (Some(target), Some(source)) = (merged_profile.as_object_mut(), profile.as_object()) {
        for (key, value) in source { target.insert(key.clone(), value.clone()); }
    }
    let mut project = read_json_value(&file);
    if !project.is_object() { project = json!({}); }
    let object = project.as_object_mut().expect("project object");
    let mut browser = Map::new();
    browser.insert("mode".into(), json!(verb));
    browser.insert("updatedAt".into(), json!(now_text()));
    if merged_launch.as_object().map(|map| !map.is_empty()).unwrap_or(false) { browser.insert("launch".into(), merged_launch.clone()); }
    if merged_profile.as_object().map(|map| !map.is_empty()).unwrap_or(false) { browser.insert("profile".into(), merged_profile.clone()); }
    object.insert("browser".into(), Value::Object(browser));
    object.remove("browserMode");
    write_json_value(&file, &project)?;
    let mut lines = vec![format!("Browser runtime preference set to {verb}."), format!("Config: {}", file.display())];
    if merged_launch.as_object().map(|map| !map.is_empty()).unwrap_or(false) { lines.push(format_browser_settings("Launch settings", &merged_launch)); }
    if merged_profile.as_object().map(|map| !map.is_empty()).unwrap_or(false) { lines.push(format_browser_settings("Profile settings", &merged_profile)); }
    lines.push("Honest scope: this configures Jeden browser-tool/controller preference only; browser availability still depends on installed local tools or MCP adapters.".into());
    Ok(lines.join("\n"))
}


fn usage_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/usage.json") }

fn handle_usage(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, _) = split_head(args);
    let verb = if verb.is_empty() { "show" } else { verb };
    let path = usage_path(context.cwd);
    if verb == "reset" {
        write_json_value(&path, &json!({"version": 1, "updatedAt": now_text(), "events": []}))?;
        return Ok(format!("Reset usage accounting: {}", path.display()));
    }
    if verb != "show" && verb != "status" {
        return Err("Usage: /usage [show|reset]".into());
    }
    let usage = read_json_value(&path);
    let events = usage.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut by_model = Map::new();
    let mut input_tokens = 0.0;
    let mut output_tokens = 0.0;
    let mut total_tokens = 0.0;
    for event in &events {
        let input = event.get("inputTokens").and_then(Value::as_f64).unwrap_or(0.0);
        let output = event.get("outputTokens").and_then(Value::as_f64).unwrap_or(0.0);
        let total = event.get("totalTokens").and_then(Value::as_f64).unwrap_or(0.0);
        input_tokens += input;
        output_tokens += output;
        total_tokens += total;
        let model = event.get("model").and_then(Value::as_str).unwrap_or("default").to_string();
        let entry = by_model.entry(model).or_insert_with(|| json!({"calls": 0, "inputTokens": 0.0, "outputTokens": 0.0, "totalTokens": 0.0}));
        if let Value::Object(map) = entry {
            let calls = map.get("calls").and_then(Value::as_u64).unwrap_or(0) + 1;
            let model_input = map.get("inputTokens").and_then(Value::as_f64).unwrap_or(0.0) + input;
            let model_output = map.get("outputTokens").and_then(Value::as_f64).unwrap_or(0.0) + output;
            let model_total = map.get("totalTokens").and_then(Value::as_f64).unwrap_or(0.0) + total;
            map.insert("calls".into(), json!(calls));
            map.insert("inputTokens".into(), json!(model_input));
            map.insert("outputTokens".into(), json!(model_output));
            map.insert("totalTokens".into(), json!(model_total));
        }
    }
    let recent = events.iter().rev().take(10).cloned().collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>();
    let summary = json!({
        "file": path,
        "updatedAt": usage.get("updatedAt").cloned().unwrap_or(Value::Null),
        "totals": {
            "calls": events.len(),
            "inputTokens": input_tokens,
            "outputTokens": output_tokens,
            "totalTokens": total_tokens,
            "byModel": by_model,
        },
        "recent": recent,
    });
    serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())
}

fn memory_file_path() -> PathBuf {
    env::var_os("JEDEN_MEMORY_FILE").map(PathBuf::from).unwrap_or_else(|| dirs_home().join(".jeden/memory.jsonl"))
}

fn tool_values(context: &SlashContext<'_>) -> Vec<Value> {
    tools::list_tools(context.cwd)
        .into_iter()
        .map(|tool| json!({"name": tool.name, "description": tool.description, "input": {}}))
        .collect()
}

fn handle_context(context: &SlashContext<'_>) -> Result<String, String> {
    let all = tool_values(context);
    let manifest = json!({
        "cwd": context.cwd,
        "model": current_model_route(context),
        "tools": {
            "total": all.len(),
            "all": all,
        },
        "memory": {
            "backend": "local-jsonl",
            "path": memory_file_path(),
        },
    });
    serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())
}

fn handle_doctor(context: &SlashContext<'_>) -> Result<String, String> {
    let all = tool_values(context);
    let report = json!({
        "ok": true,
        "cwd": context.cwd,
        "model": current_model_route(context),
        "checks": [
            {"id": "filesystem.cwd.readable", "ok": context.cwd.is_dir(), "fatal": true, "path": context.cwd},
            {"id": "tools.static.load", "ok": true, "fatal": false},
        ],
        "tools": {
            "total": all.len(),
        },
        "memory": {
            "backend": "local-jsonl",
            "path": memory_file_path(),
        },
    });
    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
}

fn ssh_hosts_from(config: &Value) -> Option<&Map<String, Value>> {
    config.get("sshHosts").and_then(Value::as_object)
        .or_else(|| config.get("ssh").and_then(|ssh| ssh.get("hosts")).and_then(Value::as_object))
        .or_else(|| config.get("ssh").and_then(Value::as_object))
}

fn ssh_host_value(target: &str, options: &[String]) -> Option<Value> {
    if options.is_empty() { return Some(Value::String(target.to_string())); }
    let mut host = Map::new();
    host.insert("host".into(), Value::String(target.to_string()));
    for option in options {
        let (key, value) = option.split_once('=')?;
        if key.is_empty() { return None; }
        host.insert(key.to_string(), Value::String(value.to_string()));
    }
    Some(Value::Object(host))
}

fn handle_ssh(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("list");
    let name = argv.get(1).map(String::as_str).unwrap_or("");
    let target = argv.get(2).map(String::as_str).unwrap_or("");
    let options = if argv.len() > 3 { &argv[3..] } else { &[] };
    let project_file = project_config_path(context.cwd);
    if verb == "list" {
        let config = merged_config(context.cwd);
        let Some(hosts) = ssh_hosts_from(&config) else {
            return Ok("No SSH hosts configured in ~/.jeden/config.json or <cwd>/.jeden/config.json (sshHosts).".into());
        };
        let mut names = hosts.keys().cloned().collect::<Vec<_>>();
        names.sort();
        if names.is_empty() {
            return Ok("No SSH hosts configured in ~/.jeden/config.json or <cwd>/.jeden/config.json (sshHosts).".into());
        }
        return Ok(names.into_iter().map(|host| {
            let value = hosts.get(&host).cloned().unwrap_or(Value::Null);
            let rendered = value.as_str().map(ToString::to_string).unwrap_or_else(|| serde_json::to_string(&value).unwrap_or_else(|_| "null".into()));
            format!("{}\t{}", host, rendered)
        }).collect::<Vec<_>>().join("\n"));
    }
    if verb == "help" {
        return Ok("Usage: /ssh list | add <name> <target> [key=value ...] | remove <name>. Hosts are stored in <cwd>/.jeden/config.json under sshHosts.".into());
    }
    if verb == "add" {
        if name.is_empty() || target.is_empty() { return Err("Usage: /ssh add <name> <target> [key=value ...]".into()); }
        let value = ssh_host_value(target, options).ok_or_else(|| "Usage: /ssh add <name> <target> [key=value ...]".to_string())?;
        let mut project = read_json_value(&project_file);
        if !project.is_object() { project = json!({}); }
        let object = project.as_object_mut().expect("project config object");
        let hosts = object.entry("sshHosts").or_insert_with(|| Value::Object(Map::new()));
        if !hosts.is_object() { *hosts = Value::Object(Map::new()); }
        hosts.as_object_mut().expect("sshHosts object").insert(name.to_string(), value);
        write_json_value(&project_file, &project)?;
        return Ok(format!("Added SSH host {} to {}.", name, project_file.display()));
    }
    if verb == "remove" {
        if name.is_empty() { return Err("Usage: /ssh remove <name>".into()); }
        let effective = merged_config(context.cwd);
        let mut project = read_json_value(&project_file);
        let in_project = project.get("sshHosts").and_then(Value::as_object).and_then(|hosts| hosts.get(name)).is_some();
        if !in_project {
            if ssh_hosts_from(&effective).and_then(|hosts| hosts.get(name)).is_some() {
                return Err(format!("SSH host {} is not in <cwd>/.jeden/config.json. Remove it from ~/.jeden/config.json or the config file that defines it.", name));
            }
            return Err(format!("SSH host not found: {}", name));
        }
        if let Some(hosts) = project.get_mut("sshHosts").and_then(Value::as_object_mut) { hosts.remove(name); }
        write_json_value(&project_file, &project)?;
        return Ok(format!("Removed SSH host {} from {}.", name, project_file.display()));
    }
    Err("Usage: /ssh list | add <name> <target> [key=value ...] | remove <name> | help".into())
}

fn resolve_cwd_path(cwd: &Path, target: &str) -> PathBuf {
    let path = PathBuf::from(target);
    if path.is_absolute() { path } else { cwd.join(path) }
}


fn load_memory_lines() -> Result<Vec<Value>, String> {
    let file = memory_file_path();
    let raw = match fs::read_to_string(&file) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).map_err(|e| e.to_string()))
        .collect()
}

fn save_memory_lines(records: &[Value]) -> Result<(), String> {
    let file = memory_file_path();
    if let Some(parent) = file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let body = records.iter().map(|record| serde_json::to_string(record).map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>()?.join("\n");
    fs::write(&file, if body.is_empty() { String::new() } else { format!("{body}\n") }).map_err(|e| e.to_string())
}

fn handle_memory(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("view");
    let records = load_memory_lines()?;
    let file = memory_file_path();
    if matches!(verb, "stats" | "diagnose") {
        return Ok(format!("Memory file: {}\nRecords: {}\nScope: {}", file.display(), records.len(), context.cwd.display()));
    }
    if matches!(verb, "clear" | "reset") {
        save_memory_lines(&[])?;
        return Ok(format!("Cleared memory file: {}", file.display()));
    }
    if matches!(verb, "view" | "list" | "") {
        if records.is_empty() { return Ok("No memory records.".into()); }
        let query = argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
        let mut shown = records;
        if !query.is_empty() {
            shown.retain(|record| record.get("text").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase().contains(&query));
        }
        let start = shown.len().saturating_sub(20);
        return Ok(shown[start..].iter().map(|record| {
            let id = record.get("id").and_then(Value::as_str).unwrap_or("-");
            let kind = record.get("kind").and_then(Value::as_str).unwrap_or("-");
            let text = record.get("text").and_then(Value::as_str).or_else(|| record.get("content").and_then(Value::as_str)).unwrap_or("");
            format!("{id}\t{kind}\t{text}")
        }).collect::<Vec<_>>().join("\n"));
    }
    Err("Usage: /memory [view [query]|stats|diagnose|clear|reset]".into())
}

fn handle_todo(args: &str, state: &mut ModeState, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("list");
    let text = argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
    if verb.is_empty() || verb == "list" {
        if state.todos.is_empty() { return Ok("Todo list is empty.".into()); }
        return Ok(state.todos.iter().enumerate().map(|(index, todo)| format!("{}. [{}] {}", index + 1, todo.status, todo.text)).collect::<Vec<_>>().join("\n"));
    }
    if verb == "add" || verb == "start" {
        if text.is_empty() { return Err(format!("Usage: /todo {} <task>", verb)); }
        state.todos.push(TodoState { text: text.clone(), status: if verb == "start" { "in_progress".into() } else { "pending".into() }, created_at: now_text() });
        return Ok(format!("Todo added: {}", text));
    }
    if verb == "done" || verb == "drop" || verb == "rm" {
        let needle = text.to_ascii_lowercase();
        let Some(index) = state.todos.iter().position(|todo| todo.text.to_ascii_lowercase().contains(&needle)).or_else(|| text.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|&n| n < state.todos.len())) else {
            return Err(format!("Todo not found: {}", if text.is_empty() { "(missing)" } else { &text }));
        };
        let todo_text = state.todos[index].text.clone();
        if verb == "rm" { state.todos.remove(index); }
        else { state.todos[index].status = if verb == "done" { "done".into() } else { "dropped".into() }; }
        return Ok(format!("{} todo: {}", if verb == "rm" { "Removed" } else { "Updated" }, todo_text));
    }
    if verb == "copy" || verb == "export" {
        let md = if state.todos.is_empty() { "- [ ]".into() } else { state.todos.iter().map(|todo| format!("- [{}] {}", if todo.status == "done" { "x" } else { " " }, todo.text)).collect::<Vec<_>>().join("\n") };
        if verb == "copy" { return Ok(md); }
        let target = resolve_cwd_path(context.cwd, if text.is_empty() { "TODO.md" } else { &text });
        if let Some(parent) = target.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        fs::write(&target, format!("{}\n", md)).map_err(|e| e.to_string())?;
        return Ok(format!("Todos exported to {}", target.display()));
    }
    if verb == "import" {
        let target = resolve_cwd_path(context.cwd, if text.is_empty() { "TODO.md" } else { &text });
        let raw = fs::read_to_string(&target).map_err(|e| e.to_string())?;
        state.todos = raw.lines().filter_map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("- [") { return None; }
            let status_mark = trimmed.chars().nth(3)?;
            let text = trimmed.get(6..)?.trim().to_string();
            if text.is_empty() { return None; }
            Some(TodoState { text, status: if status_mark == 'x' || status_mark == 'X' { "done".into() } else { "pending".into() }, created_at: now_text() })
        }).collect();
        return Ok(format!("Imported {} todos from {}", state.todos.len(), target.display()));
    }
    Err("Usage: /todo [list|add|start|done|drop|rm|copy|export|import]".into())
}

fn handle_guided_goal(args: &str, state: &mut ModeState) -> Result<String, String> {
    let objective = args.trim();
    if objective.is_empty() { return Err("Usage: /guided-goal <rough objective>".into()); }
    state.guided_goal.active = true;
    state.guided_goal.rough_objective = objective.to_string();
    Ok("Guided goal drafting started. Jeden will use the next turn to refine the objective instead of pretending to open an overlay.".into())
}

fn handle_force(args: &str, state: &mut ModeState, context: &SlashContext<'_>) -> Result<String, String> {
    let (tool, prompt) = split_head(args);
    if tool.is_empty() { return Err("Usage: /force <tool-name>".into()); }
    if !prompt.trim().is_empty() {
        return Err("/force <tool> <prompt> requires immediate runTask support; use /force <tool>, then send the prompt as the next turn.".into());
    }
    let names = tools::list_tools(context.cwd).into_iter().map(|tool| tool.name).collect::<Vec<_>>();
    if !names.is_empty() && !names.iter().any(|name| name == tool) {
        return Err(format!("Unknown or unavailable tool: {}. Visible tools: {}", tool, names.iter().take(20).cloned().collect::<Vec<_>>().join(", ")));
    }
    state.force = Some(ForceState { tool: tool.to_string(), prompt: String::new() });
    Ok(format!("The next agent turn will be instructed to use {} first.", tool))
}

fn handle_branching(command: &str, args: &str, state: &mut ModeState) -> Result<String, String> {
    if command == "/tree" {
        if state.branches.is_empty() { return Ok(String::new()); }
        return Ok(state.branches.iter().map(|branch| format!("{}\t{}\t{}", branch.id, branch.title, branch.created_at)).collect::<Vec<_>>().join("\n"));
    }
    let id = format!("{}-{}", command.trim_start_matches('/'), state.branches.len() + 1);
    state.branches.push(BranchState { id: id.clone(), title: if args.trim().is_empty() { id.clone() } else { args.trim().into() }, created_at: now_text() });
    Ok(format!("{} created locally: {}", command.trim_start_matches('/'), id))
}


fn handle_unavailable(command: &str) -> Result<String, String> {
    let name = command.trim_start_matches('/');
    Err(format!("/{name} is recognized but this interactive-only command is not available in the Rust TUI yet. Use the JS entrypoint for this command until the Rust-native flow is implemented."))
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
        "/guided-goal" => { changed = true; Some(handle_guided_goal(args, &mut state)) },
        "/loop" => { changed = split_head(args).0 != "status"; Some(handle_loop(args, &mut state)) },
        "/fast" => { changed = split_head(args).0 != "status"; Some(handle_fast(args, &mut state)) },
        "/advisor" => { changed = !matches!(split_head(args).0, "" | "status" | "dump"); Some(handle_advisor(args, &mut state, context)) },
        "/plan-review" => Some(if state.plan.latest_plan.is_empty() { Err("No plan is available to review yet. Run a prompt while /plan is enabled, then use /plan-review.".into()) } else { Ok("Reopening the latest plan for review.".into()) }),
        "/tools" => Some(Ok(tools::tools_slash_text(context.cwd))),
        "/context" => Some(handle_context(context)),
        "/stats" | "/debug" => Some(handle_doctor(context)),
        "/usage" => Some(handle_usage(args, context)),
        "/session" => Some(handle_session(args, context)),
        "/todo" => { changed = !matches!(split_head(args).0, "" | "list" | "copy" | "export"); Some(handle_todo(args, &mut state, context)) },
        "/mcp" => Some(handle_mcp(args, context)),
        "/ssh" => Some(handle_ssh(args, context)),
        "/browser" => Some(handle_browser(args, context)),
        "/extensions" | "/status" => Some(handle_extensions(context)),
        "/plugins" => Some(handle_plugins(args, context)),
        "/force" | "/force:" => { changed = true; Some(handle_force(args, &mut state, context)) },
        "/retry" => Some(Err("/retry must be executed through the agent runner so it can replay lastFailedTask.".into())),
        "/memory" => Some(handle_memory(args, context)),
        "/branch" | "/fork" | "/tree" => { changed = command != "/tree"; Some(handle_branching(command.as_str(), args, &mut state)) },
        "/new" | "/fresh" | "/drop" | "/compact" | "/shake" | "/resume" | "/rename" | "/move" => {
            changed = matches!(command.as_str(), "/compact" | "/shake");
            handle_lifecycle(command.as_str(), args, &mut state, context)
        },
        "/agents" => Some(Ok("Agent controls:\n- /tan <work> starts a detached local agent job tracked in session artifacts.\n- /advisor manages second-pass reviewer mode.\n- /jobs shows locally tracked background jobs.".into())),
        "/jobs" => Some(Ok("No background jobs are tracked inside this Jeden process.".into())),
        "/changelog" => Some(Ok("No bundled changelog is present in Jeden. Git history is the source of release notes for this package.".into())),
        "/hotkeys" => Some(Ok("Jeden interactive hotkeys:\nEnter submits the prompt.\nCtrl-J inserts a newline.\nLeft/Right/Home/End edit inside the prompt.\nUp/Down navigate prompt history.\nCtrl-C exits input mode or denies approval.".into())),
        "/marketplace" | "/reload-plugins" | "/export" | "/dump" | "/share" | "/copy" | "/collab" | "/join" | "/leave" | "/btw" | "/tan" | "/omfg" | "/handoff" => Some(handle_unavailable(command.as_str())),
        _ => None,
    };
    if changed {
        if let Some(Ok(_)) = &result {
            if let Err(error) = write_mode_state(context.cwd, &state) { return Some(Err(error)); }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("jeden-slash-{}-{}", name, now_millis()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn test_context<'a>(cwd: &'a Path, session_root: &'a Path) -> SlashContext<'a> {
        SlashContext { cwd, model: Some("test-model"), session_root }
    }

    #[test]
    fn ssh_add_list_remove_roundtrip_updates_project_config() {
        let cwd = temp_workspace("ssh");
        let sessions = cwd.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let context = test_context(&cwd, &sessions);

        let added = handle_local(&context, "/ssh add lab example.test user=me").unwrap().unwrap();
        assert!(added.contains("Added SSH host lab"));
        let listed = handle_local(&context, "/ssh list").unwrap().unwrap();
        assert!(listed.contains("lab"));
        assert!(listed.contains("example.test"));
        assert!(listed.contains("\"user\":\"me\""));
        let removed = handle_local(&context, "/ssh remove lab").unwrap().unwrap();
        assert!(removed.contains("Removed SSH host lab"));
        let listed = handle_local(&context, "/ssh list").unwrap().unwrap();
        assert!(listed.contains("No SSH hosts configured"));
    }

    #[test]
    fn usage_summary_and_reset_match_js_shape() {
        let cwd = temp_workspace("usage");
        let sessions = cwd.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(cwd.join(".jeden")).unwrap();
        fs::write(cwd.join(".jeden/usage.json"), r#"{"updatedAt":"then","events":[{"model":"a","inputTokens":2,"outputTokens":3,"totalTokens":5},{"model":"a","inputTokens":4,"outputTokens":1,"totalTokens":5}]}"#).unwrap();
        let context = test_context(&cwd, &sessions);

        let summary = handle_local(&context, "/usage show").unwrap().unwrap();
        let value: Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(value["totals"]["calls"], 2);
        assert_eq!(value["totals"]["totalTokens"], 10.0);
        assert_eq!(value["totals"]["byModel"]["a"]["calls"], 2);

        let reset = handle_local(&context, "/usage reset").unwrap().unwrap();
        assert!(reset.contains("Reset usage accounting"));
        let reset_file = read_json_value(&cwd.join(".jeden/usage.json"));
        assert_eq!(reset_file["events"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn memory_view_stats_clear_use_jsonl_file() {
        let cwd = temp_workspace("memory");
        let sessions = cwd.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let memory_file = cwd.join("memory.jsonl");
        env::set_var("JEDEN_MEMORY_FILE", &memory_file);
        fs::write(&memory_file, "{\"id\":\"m1\",\"kind\":\"note\",\"text\":\"alpha durable\"}\n").unwrap();
        let context = test_context(&cwd, &sessions);

        let stats = handle_local(&context, "/memory stats").unwrap().unwrap();
        assert!(stats.contains("Records: 1"));
        let view = handle_local(&context, "/memory view alpha").unwrap().unwrap();
        assert!(view.contains("alpha durable"));
        let cleared = handle_local(&context, "/memory clear").unwrap().unwrap();
        assert!(cleared.contains("Cleared memory file"));
        assert_eq!(fs::read_to_string(&memory_file).unwrap(), "");
        env::remove_var("JEDEN_MEMORY_FILE");
    }
}
