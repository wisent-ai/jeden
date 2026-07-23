use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::slash::common::split_head;
use crate::slash::state::{read_mode_state, ModeState};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

/// Roles hub: one row per model role, each opening the existing view that
/// owns the role (`/model`, `/fast`, `/advisor`). Details report live state.
pub(crate) fn roles_picker(state: &ModeState, context: &SlashContext<'_>) -> PickerSpec {
    let model = context
        .model
        .map(str::to_string)
        .or_else(|| crate::load_config(context.cwd).model)
        .or_else(|| std::env::var("JEDEN_MODEL").ok())
        .filter(|model| !model.trim().is_empty());
    let model_detail = match &model {
        Some(route) => format!("Current route: {route} · /model to change"),
        None => "No model route selected · /model to choose".to_string(),
    };
    let tier = if state.fast.service_tier.trim().is_empty() {
        "priority"
    } else {
        state.fast.service_tier.trim()
    };
    let fast_detail = if state.fast.enabled {
        format!("Enabled · service tier: {tier} · /fast to configure")
    } else {
        "Disabled · /fast to configure".to_string()
    };
    let advisor = &state.advisor;
    let advisor_detail = if advisor.enabled {
        format!(
            "Enabled · reviewer route: {} · /advisor to configure",
            super::advisor_model_label(advisor, context)
        )
    } else {
        "Disabled · /advisor to configure".to_string()
    };
    PickerSpec::new(
        "Model roles",
        vec![
            PickerItem::action("default model", "/roles model")
                .detail(model_detail)
                .badge("MODEL"),
            PickerItem::action("fast tier", "/roles fast")
                .detail(fast_detail)
                .badge(if state.fast.enabled { "ON" } else { "OFF" }),
            PickerItem::action("advisor", "/roles advisor")
                .detail(advisor_detail)
                .badge(if advisor.enabled { "ON" } else { "OFF" }),
        ],
    )
}

/// Non-interactive `/roles`: the same hub rows rendered as text.
pub(crate) fn handle_roles(context: &SlashContext<'_>) -> Result<String, String> {
    let state = read_mode_state(context.cwd);
    Ok(crate::tui::CommandOutcome::Picker(roles_picker(&state, context)).into_text())
}

pub(crate) fn advisor_picker(state: &ModeState, context: &SlashContext<'_>) -> PickerSpec {
    let advisor = &state.advisor;
    let route = super::advisor_model_label(advisor, context);
    let mut items = vec![
        PickerItem::action(
            if advisor.enabled {
                "Disable advisor"
            } else {
                "Enable advisor"
            },
            if advisor.enabled {
                "/advisor off"
            } else {
                "/advisor on"
            },
        )
        .detail(format!("Reviewer route: {}", route))
        .badge(if advisor.enabled { "ON" } else { "OFF" }),
        PickerItem::action("Show advisor status", "/advisor status")
            .detail("Show reviewer state, route, and note availability"),
    ];
    if advisor.last_review.is_some() {
        items.push(
            PickerItem::action("Show advisor notes", "/advisor dump")
                .detail("Display the latest reviewer notes")
                .badge("AVAILABLE"),
        );
        items.push(
            PickerItem::action("Show raw advisor notes", "/advisor dump raw")
                .detail("Display the complete reviewer response as JSON"),
        );
    }
    PickerSpec::new("Advisor workflow", items)
}

pub(crate) fn approval_picker(state: &ModeState) -> PickerSpec {
    let current = if state.tools.approval_mode.trim().is_empty() {
        "default"
    } else {
        state.tools.approval_mode.as_str()
    };
    let mut items = vec![
        PickerItem::action("Show approval policy", "/approval status")
            .detail("Show the global mode and per-tool policies")
            .badge(current),
    ];
    for mode in ["always-ask", "write", "yolo"] {
        items.push(
            PickerItem::action(
                format!("Use {} approval mode", mode),
                format!("/approval mode {}", mode),
            )
            .detail("Set the global approval mode")
            .badge(if mode == current { "CURRENT" } else { "MODE" })
            .disabled(mode == current),
        );
    }
    for (tool, active) in &state.tools.approval {
        for policy in ["allow", "deny", "prompt"] {
            items.push(
                PickerItem::action(
                    format!("Set {} to {}", tool, policy),
                    format!("/approval {} {}", tool, policy),
                )
                .detail(format!("Current {} policy: {}", tool, active))
                .badge(if policy == active { "CURRENT" } else { "TOOL" })
                .disabled(policy == active),
            );
        }
    }
    items.push(
        PickerItem::action("Reset approval policy", "/approval reset")
            .detail("Clear the global mode and every tool override")
            .badge("DESTRUCTIVE"),
    );
    PickerSpec::new("Approval workflow", items)
}

pub(crate) fn tree_picker(state: &ModeState) -> PickerSpec {
    let mut items = vec![PickerItem::action("Show branch tree", "/tree show")
        .detail(if state.branches.is_empty() {
            "No recorded branches"
        } else {
            "Show every recorded branch"
        })
        .badge(if state.branches.is_empty() {
            "EMPTY"
        } else {
            "AVAILABLE"
        })];
    for branch in &state.branches {
        items.push(
            PickerItem::action(
                if branch.title.trim().is_empty() {
                    branch.id.clone()
                } else {
                    branch.title.clone()
                },
                format!("/resume {}", branch.path),
            )
            .detail(format!(
                "{} · {} · {}",
                branch.id, branch.created_at, branch.path
            ))
            .badge("BRANCH"),
        );
    }
    PickerSpec::new("Branch tree", items)
}

/// Only the newest sessions get a first-task preview: each preview costs one
/// transcript open, which adds up over hundreds of session directories.
const MESSAGE_PREVIEW_SESSIONS: usize = 50;
const MESSAGE_PREVIEW_CHARS: usize = 60;

fn started_epoch(started: &str) -> Option<u64> {
    let trimmed = started.trim();
    if let Ok(epoch) = trimmed.parse::<u64>() {
        return Some(epoch);
    }
    parse_rfc3339_epoch(trimmed)
}

/// Howard Hinnant's days-from-civil; no date crate is available here.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Epoch seconds from `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`; anything else
/// is unparseable and yields no age.
fn parse_rfc3339_epoch(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year: i64 = value.get(0..4)?.parse().ok()?;
    let month: u32 = value.get(5..7)?.parse().ok()?;
    let day: u32 = value.get(8..10)?.parse().ok()?;
    let hour: i64 = value.get(11..13)?.parse().ok()?;
    let minute: i64 = value.get(14..16)?.parse().ok()?;
    let second: i64 = value.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let rest = value[19..].trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    let zone = rest.as_bytes();
    let offset: i64 = if rest.eq_ignore_ascii_case("z") || rest.is_empty() {
        0
    } else if zone.len() == 6 && (zone[0] == b'+' || zone[0] == b'-') && zone[3] == b':' {
        let sign = if zone[0] == b'-' { -1 } else { 1 };
        let hours: i64 = rest.get(1..3)?.parse().ok()?;
        let minutes: i64 = rest.get(4..6)?.parse().ok()?;
        sign * (hours * 3_600 + minutes * 60)
    } else {
        return None;
    };
    let epoch =
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second - offset;
    u64::try_from(epoch).ok()
}

fn relative_age(started: &str) -> Option<String> {
    let epoch = started_epoch(started)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let elapsed = now.saturating_sub(epoch);
    Some(if elapsed < 60 {
        format!("{elapsed}s")
    } else if elapsed < 3_600 {
        format!("{}m", elapsed / 60)
    } else if elapsed < 48 * 3_600 {
        format!("{}h", elapsed / 3_600)
    } else {
        format!("{}d", elapsed / 86_400)
    })
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// First user task/message in the session transcript, reading only until the
/// first `user` event line. Handles both V2 (`payload.type`) and legacy
/// (`type`) transcript lines.
fn first_user_task(session_dir: &Path) -> Option<String> {
    let file = fs::File::open(session_dir.join("transcript.jsonl")).ok()?;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let payload = value.get("payload").unwrap_or(&value);
        if payload.get("type").and_then(serde_json::Value::as_str) != Some("user") {
            continue;
        }
        let data = payload.get("data")?;
        let text = ["task", "content", "text"]
            .iter()
            .find_map(|key| data.get(key).and_then(serde_json::Value::as_str))?;
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() {
            return Some(truncate_chars(&collapsed, MESSAGE_PREVIEW_CHARS));
        }
    }
    None
}

fn session_items(session_root: &Path) -> Vec<PickerItem> {
    let mut entries = Vec::new();
    if let Ok(read_dir) = fs::read_dir(session_root) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            let metadata = fs::read_to_string(path.join("state.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .unwrap_or_default();
            let name = metadata
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(&id)
                .to_string();
            let workspace = metadata
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let started = metadata
                .get("startedAt")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            entries.push((path, id, name, workspace, started));
        }
    }
    // Newest first by start time; only these get a transcript preview read.
    let mut recency: Vec<usize> = (0..entries.len()).collect();
    recency.sort_by(|left, right| {
        started_epoch(&entries[*right].4).cmp(&started_epoch(&entries[*left].4))
    });
    let preview: std::collections::HashSet<usize> =
        recency.into_iter().take(MESSAGE_PREVIEW_SESSIONS).collect();
    let mut items = Vec::new();
    for (index, (path, id, name, workspace, started)) in entries.iter().enumerate() {
        let mut parts = vec![workspace.clone(), started.clone()];
        if let Some(age) = relative_age(started) {
            parts.push(age);
        }
        if preview.contains(&index) {
            if let Some(task) = first_user_task(path) {
                parts.push(task);
            }
        }
        let detail = parts
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        items.push(
            PickerItem::action(name.clone(), format!("/resume {}", id))
                .detail(detail)
                .badge("SESSION"),
        );
    }
    items.sort_by(|left, right| right.label.cmp(&left.label));
    items
}

pub(crate) fn session_picker(context: &SlashContext<'_>) -> PickerSpec {
    let mut items = vec![
        PickerItem::action("Show current session info", "/session info")
            .detail(format!("Workspace: {}", context.cwd.display()))
            .badge("CURRENT"),
    ];
    items.extend(session_items(context.session_root));
    PickerSpec::new("Session workflow", items)
}

pub(crate) fn lifecycle_picker(state: &ModeState, context: &SlashContext<'_>) -> PickerSpec {
    let mut items = vec![
        PickerItem::action("Start a fresh conversation", "/new now")
            .detail("Clear prior turns and start a new session"),
        PickerItem::action("Shake context", "/shake elide")
            .detail(if state.shake.trim().is_empty() {
                "Apply the elide context strategy"
            } else {
                "Reapply the elide context strategy"
            })
            .badge(if state.shake.trim().is_empty() {
                "OFF"
            } else {
                "ACTIVE"
            }),
    ];
    items.push(
        PickerItem::action("Rename current session", "/rename ")
            .detail("Edit the new session name before submitting")
            .badge("INPUT")
            .prefill(),
    );
    items.push(
        PickerItem::action("Move workspace", "/move ")
            .detail("Edit the destination directory before submitting")
            .badge("INPUT")
            .prefill(),
    );
    items.extend(session_items(context.session_root));
    items.push(
        PickerItem::action("Drop current session", "/drop confirm")
            .detail("Delete the active session and start fresh")
            .badge("DESTRUCTIVE"),
    );
    PickerSpec::new("Session lifecycle", items)
}

/// List every session directory under `session_root`, one per line. The prior
/// "most recent N" display cap was an unconsented numeric literal and has been
/// removed; all sessions are listed.
fn list_sessions(session_root: &Path) -> String {
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(session_root) {
        for entry in entries.flatten() {
            rows.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    if rows.is_empty() {
        "No sessions found.".into()
    } else {
        rows.join("\n")
    }
}

fn session_path(session_root: &Path, id_or_path: &str) -> PathBuf {
    if id_or_path.contains('/') {
        PathBuf::from(id_or_path)
    } else {
        session_root.join(id_or_path)
    }
}

pub(crate) fn handle_session(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, _) = split_head(args);
    if verb.is_empty() || verb == "info" {
        return Ok(format!("Session: rust one-shot slash invocation\nWorkspace: {}\nSession root: {}\nRecorder: not active in this non-interactive Rust command", context.cwd.display(), context.session_root.display()));
    }
    if verb == "delete" {
        return Err("Refusing to delete the active session from inside itself. Exit Jeden, then remove the session directory explicitly if you still want this destructive action.".into());
    }
    Err("Usage: /session [info|delete]".into())
}

pub(crate) fn handle_lifecycle(
    command: &str,
    args: &str,
    state: &mut ModeState,
    context: &SlashContext<'_>,
) -> Option<Result<String, String>> {
    match command {
        "/new" | "/fresh" => Some(Ok("Started a fresh logical turn context. Provider stream state is reset for the next prompt in this Jeden process.".into())),
        "/drop" => Some(Err("Refusing to delete the active session from inside itself. Use /new for a fresh context or exit and remove the session directory explicitly.".into())),
        "/shake" => {
            state.shake = if args.trim().is_empty() { "elide".into() } else { args.trim().into() };
            Some(Ok(format!("Shake mode applied locally: {}. Subsequent prompts will instruct the model to avoid relying on heavy prior artifacts unless re-read.", state.shake)))
        },
        "/resume" => {
            let (id, _) = split_head(args);
            if id.is_empty() { Some(Ok(list_sessions(context.session_root))) }
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
