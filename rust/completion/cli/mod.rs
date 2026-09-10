use super::{model::*, operations, store};
use crate::Args;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) const USAGE: &str = "jeden todo [list|add <request>|pause <id>|resume <id>|cancel <id>|continue] [--session <id-or-path>] [--revision <n> --reason <text>] [--json]";

pub(crate) fn workspace(session: &Path) -> Result<PathBuf, String> {
    let bytes = std::fs::read(session.join("state.json"))
        .map_err(|error| format!("cannot read session workspace: {error}"))?;
    let state: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    state.get("cwd").and_then(Value::as_str).filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from).ok_or_else(|| "session has no recorded workspace".into())
}

pub(crate) fn active_session(cwd: &Path, selected: Option<&str>) -> Result<PathBuf, String> {
    let path = match selected {
        Some(selected) => crate::cli::sessions::session_dir_for(selected),
        None => crate::slash::read_mode_state(cwd).last_session_path
            .ok_or("no active session; use --session <id-or-path>")?,
    };
    store::read(&path)?;
    Ok(path)
}

pub(crate) fn render(state: &CompletionState) -> String {
    let mut lines = vec![format!("Tasks: {} (revision {})", state.status(), state.revision)];
    if let Some(blocker) = &state.blocker {
        lines.push(format!("{}: {}", blocker.operation, blocker.message));
    }
    for request in &state.requests {
        lines.push(format!("Request {} [{}]: {}", request.id,
            if request.paused { "paused" } else if request.coverage_verified { "covered" }
            else if request.planned { "planned" } else { "unplanned" }, request.prompt));
        for task in state.tasks.iter().filter(|task| task.request_id == request.id) {
            lines.push(format!("  {} [{}] {}", task.id,
                serde_json::to_value(task.status).unwrap_or_default().as_str().unwrap_or("unknown"), task.text));
            for criterion in &task.criteria {
                lines.push(format!("    Acceptance: {criterion}"));
            }
            if let Some(reason) = &task.reason {
                lines.push(format!("    {reason}"));
            }
            if let Some(verification) = &task.verification {
                for evidence in &verification.evidence {
                    lines.push(format!("    Evidence: {}#{}", evidence.session_path, evidence.event_id));
                }
            }
        }
    }
    lines.join("\n")
}

pub(crate) fn command(args: &Args) -> Result<String, String> {
    execute(&args.cwd, &args.positionals, args.json, Some(args))
}

pub(crate) fn execute(cwd: &Path, arguments: &[String], mut json: bool, run_args: Option<&Args>) -> Result<String, String> {
    let mut selected = None;
    let mut revision = None;
    let mut reason = None;
    let mut words = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--session" => selected = Some(arguments.next().ok_or("--session requires an id or path")?.as_str()),
            "--revision" => revision = Some(arguments.next().ok_or("--revision requires a number")?
                .parse::<u64>().map_err(|_| "revision must be an unsigned integer")?),
            "--reason" => reason = Some(arguments.next().ok_or("--reason requires an explanation")?.as_str()),
            "--json" => json = true,
            "--help" => return Ok(USAGE.into()),
            flag if flag.starts_with("--") => return Err(format!("unknown todo option: {flag}")),
            _ => words.push(argument.as_str()),
        }
    }
    let (action, remaining) = words.split_first().map(|(action, rest)| (*action, rest)).unwrap_or(("list", &[]));
    if !matches!(action, "list" | "add" | "pause" | "resume" | "cancel" | "continue") {
        return Err(format!("{USAGE}\nTask completion requires independent verification; there is no operator done command."));
    }
    if selected.is_none() {
        super::migrate_workspace(cwd, None)?;
    }
    let session = if action == "add" && selected.is_none() && crate::slash::read_mode_state(cwd).last_session_path.is_none() {
        let conversation = crate::agent::Conversation::new(cwd)?;
        let path = conversation.session_path();
        crate::agent::update_last_session_path(cwd, &path)?;
        path
    } else {
        active_session(cwd, selected)?
    };
    let state = match action {
        "list" if remaining.is_empty() => store::read(&session)?,
        "add" if !remaining.is_empty() => {
            let request = remaining.join(" ");
            operations::capture_request(&session, &workspace(&session)?, &request)?.1
        }
        "pause" | "resume" | "cancel" => {
            let [task_id] = remaining else { return Err(USAGE.into()); };
            operations::operator_control(&session, task_id, action,
                reason.ok_or("task control requires --reason")?,
                revision.ok_or("task control requires --revision")?)?
        }
        "continue" if remaining.is_empty() => {
            let mut args = run_args.cloned().ok_or("Use jeden todo continue with the current execution grants.")?;
            args.cwd = workspace(&session)?;
            let mut conversation = crate::agent::Conversation::open(&args.cwd, &session)?;
            let mut hooks = crate::agent::RunHooks::inert();
            let text = conversation.continue_work(&args, &mut hooks)?;
            let completion = conversation.completion_state()?;
            return if json {
                serde_json::to_string_pretty(&serde_json::json!({"text": text, "sessionPath": conversation.session_path(), "completion": completion})).map_err(|error| error.to_string())
            } else { Ok(text) };
        }
        _ => return Err(USAGE.into()),
    };
    let value = operations::snapshot_value(&state);
    if action != "list" {
        crate::cli::sessions::append_ledger_entry(&session, crate::agent::now_stamp(), "completion_state", value.clone())?;
    }
    if json { serde_json::to_string_pretty(&value).map_err(|error| error.to_string()) }
    else { Ok(render(&state)) }
}
