use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::slash::common::{file_url, now_text, read_json_value, resolve_cwd_path, split_args};
use crate::slash::state::mode_state_path;
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

pub(crate) mod clipboard;
pub(crate) mod collab;

pub(super) fn collab_picker(context: &SlashContext<'_>) -> PickerSpec {
    collab::build_collab_picker(context)
}

pub(super) fn join_picker(context: &SlashContext<'_>) -> PickerSpec {
    collab::build_join_picker(context)
}

pub(super) fn leave_picker(context: &SlashContext<'_>) -> PickerSpec {
    collab::build_leave_picker(context)
}

pub(super) fn copy_picker() -> PickerSpec {
    clipboard::build_copy_picker()
}

use crate::slash::plugins::marketplace::sanitize_marketplace_name;
use clipboard::write_clipboard;

fn slash_session_dir(context: &SlashContext<'_>, id_or_path: &str) -> Result<PathBuf, String> {
    let target = if id_or_path.trim().is_empty() {
        read_json_value(&mode_state_path(context.cwd))
            .get("lastSessionPath")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or("No current Rust session is recorded yet; pass a session id or path.")?
    } else {
        id_or_path.trim().to_string()
    };
    let raw_path = PathBuf::from(&target);
    let path = if target.contains('/') {
        if raw_path.is_absolute() {
            raw_path
        } else {
            context.cwd.join(raw_path)
        }
    } else {
        context.session_root.join(target)
    };
    if !path.exists() {
        return Err(format!("session not found: {}", path.display()));
    }
    Ok(path)
}

fn slash_session_value(context: &SlashContext<'_>, id_or_path: &str) -> Result<Value, String> {
    let dir = slash_session_dir(context, id_or_path)?;
    crate::cli::sessions::read_session_value(&slash_command_path(&dir))
}

fn slash_session_text(session: &Value) -> String {
    let mut out = vec![
        format!(
            "Session: {}",
            session
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("session")
        ),
        format!(
            "Path: {}",
            session.get("path").and_then(Value::as_str).unwrap_or("")
        ),
        String::new(),
    ];
    for event in session
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        out.push(
            format!(
                "## {} {}",
                event.get("ts").and_then(Value::as_str).unwrap_or(""),
                event.get("type").and_then(Value::as_str).unwrap_or("")
            )
            .trim()
            .to_string(),
        );
        out.push(
            serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null))
                .unwrap_or_else(|_| "{}".into()),
        );
        out.push(String::new());
    }
    out.join("\n")
}

fn slash_html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn slash_session_export(session: &Value, format: &str) -> Result<String, String> {
    if format == "json" {
        return Ok(serde_json::to_string_pretty(session).map_err(|e| e.to_string())? + "\n");
    }
    if format == "markdown" || format == "md" {
        let mut out = format!(
            "# Jeden session {}\n\n{}\n\n",
            session
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("session"),
            session.get("path").and_then(Value::as_str).unwrap_or("")
        );
        for event in session
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let label = format!(
                "{} {}",
                event.get("ts").and_then(Value::as_str).unwrap_or(""),
                event.get("type").and_then(Value::as_str).unwrap_or("")
            )
            .trim()
            .to_string();
            let data = serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null))
                .unwrap_or_else(|_| "{}".into());
            out.push_str(&format!("## {}\n\n```json\n{}\n```\n\n", label, data));
        }
        return Ok(out);
    }
    if format == "html" {
        let id = slash_html_escape(
            session
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("session"),
        );
        let path = slash_html_escape(session.get("path").and_then(Value::as_str).unwrap_or(""));
        let mut body = String::new();
        for event in session
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let label = slash_html_escape(
                format!(
                    "{} {}",
                    event.get("ts").and_then(Value::as_str).unwrap_or(""),
                    event.get("type").and_then(Value::as_str).unwrap_or("")
                )
                .trim(),
            );
            let data = slash_html_escape(
                &serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null))
                    .unwrap_or_else(|_| "{}".into()),
            );
            body.push_str(&format!(
                "<section><h2>{}</h2><pre>{}</pre></section>\n",
                label, data
            ));
        }
        return Ok(format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>Jeden session {}</title></head><body><h1>Jeden session {}</h1><p>{}</p>{}</body></html>\n", id, id, path, body));
    }
    Err(format!("unsupported session export format: {}", format))
}

fn current_session_picker_detail(context: &SlashContext<'_>) -> String {
    match slash_session_dir(context, "") {
        Ok(path) => {
            let id = path
                .file_name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_else(|| path.to_string_lossy());
            format!("Current session {id} at {}", path.display())
        }
        Err(error) => error,
    }
}

fn slash_command_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn current_session_command(context: &SlashContext<'_>, command: &str) -> Option<String> {
    slash_session_dir(context, "")
        .ok()
        .map(|path| format!("{command} {}", slash_command_path(&path)))
}

pub(super) fn dump_picker(context: &SlashContext<'_>) -> PickerSpec {
    let command = current_session_command(context, "/dump");
    PickerSpec::new(
        "Dump session",
        vec![PickerItem::action(
            "Print current session transcript",
            command.clone().unwrap_or_default(),
        )
        .detail(current_session_picker_detail(context))
        .badge("text")
        .disabled(command.is_none())],
    )
}

pub(super) fn export_picker(context: &SlashContext<'_>) -> PickerSpec {
    let session = slash_session_dir(context, "").ok();
    let detail = current_session_picker_detail(context);
    let command = |prefix: &str| {
        session
            .as_ref()
            .map(|path| {
                let path = slash_command_path(path)
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"");
                format!("{prefix} \"{path}\"")
            })
            .unwrap_or_default()
    };
    PickerSpec::new(
        "Export session",
        vec![
            PickerItem::action("Print JSON export", command("/export"))
                .detail(&detail)
                .badge("JSON")
                .disabled(session.is_none()),
            PickerItem::action("Print Markdown export", command("/export --markdown"))
                .detail(&detail)
                .badge("Markdown")
                .disabled(session.is_none()),
            PickerItem::action("Print HTML export", command("/export --html"))
                .detail(detail)
                .badge("HTML")
                .disabled(session.is_none()),
        ],
    )
}

pub(super) fn share_picker(context: &SlashContext<'_>) -> PickerSpec {
    let available = slash_session_dir(context, "").is_ok();
    let detail = current_session_picker_detail(context);
    PickerSpec::new(
        "Share session",
        vec![
            PickerItem::action("Create encrypted share bundle", "/share bundle")
                .detail(&detail)
                .badge("writes artifact")
                .disabled(!available),
            PickerItem::action("Create bundle and copy share URL", "/share --copy")
                .detail(detail)
                .badge("writes artifact + clipboard")
                .disabled(!available),
        ],
    )
}

pub(super) fn tan_picker(context: &SlashContext<'_>) -> PickerSpec {
    PickerSpec::new(
        "Start background agent job",
        vec![PickerItem::action("Enter background work", "/tan ")
            .detail(format!(
                "Edit the work request before submitting. {}",
                current_session_picker_detail(context)
            ))
            .badge("INPUT")
            .prefill()],
    )
}

pub(super) fn omfg_picker(context: &SlashContext<'_>) -> PickerSpec {
    PickerSpec::new(
        "Forge a local rule",
        vec![PickerItem::action("Describe the local rule", "/omfg ")
            .detail(format!(
                "Edit the complaint before submitting. Rules file: {}",
                context.cwd.join(".jeden/rules.jsonl").display()
            ))
            .badge("INPUT")
            .prefill()],
    )
}

pub(crate) fn handle_dump(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    Ok(slash_session_text(&slash_session_value(
        context,
        args.trim(),
    )?))
}

pub(crate) fn handle_export(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let mut id = String::new();
    let mut format = "json".to_string();
    let mut output: Option<String> = None;
    for arg in argv {
        if arg == "--html" {
            format = "html".into();
        } else if arg == "--markdown" || arg == "--md" {
            format = "markdown".into();
        } else if id.is_empty()
            && !arg.starts_with("--")
            && slash_session_dir(context, &arg).is_ok()
        {
            id = arg;
        } else {
            output = Some(arg);
        }
    }
    let payload = slash_session_export(&slash_session_value(context, &id)?, &format)?;
    if let Some(path) = output {
        let target = resolve_cwd_path(context.cwd, &path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&target, &payload).map_err(|e| e.to_string())?;
        Ok(target.display().to_string())
    } else {
        Ok(payload)
    }
}

pub(crate) fn handle_share(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let copy_link = argv
        .iter()
        .any(|arg| matches!(arg.as_str(), "copy" | "--copy" | "--clipboard"));
    let session = slash_session_value(context, "")?;
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("session");
    let created_at = now_text();
    let plain = serde_json::to_vec_pretty(
        &json!({ "kind": "jeden-session", "createdAt": created_at, "session": session }),
    )
    .map_err(|e| e.to_string())?;
    // Delegate the AES-256-GCM key/nonce/tag sizing to crate::collab, which
    // encapsulates those cryptographic constants. The framed blob carries the
    // nonce, ciphertext and tag together; the key is returned only in the URL
    // fragment and is never written into the bundle.
    let (_room, key) = crate::collab::new_room_and_key();
    let blob = crate::collab::encrypt_blob(&key, &plain)?;
    let session_dir = slash_session_dir(context, "")?;
    let artifact_dir = session_dir.join("artifacts");
    fs::create_dir_all(&artifact_dir).map_err(|e| e.to_string())?;
    let file = artifact_dir.join(format!(
        "share-{}-{}.jeden-share",
        sanitize_marketplace_name(id),
        created_at
    ));
    let bundle = json!({
        "kind": "jeden-encrypted-share",
        "backend": "file",
        "durable": true,
        "algorithm": "AES-256-GCM",
        "createdAt": created_at,
        "sessionId": id,
        "blob": blob,
        "note": "Durable encrypted session bundle. The decryption key is carried only in the returned URL fragment; keep the fragment private."
    });
    fs::write(
        &file,
        serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())? + "\n",
    )
    .map_err(|e| e.to_string())?;
    let url = format!(
        "{}#key={}",
        file_url(&file),
        crate::collab::encode_key(&key)
    );
    let copy_status = if copy_link {
        match write_clipboard(&url) {
            Ok(command) => format!("Copied share URL to clipboard with {}.", command),
            Err(error) => format!("Could not copy share URL to clipboard: {}", error),
        }
    } else {
        "Add `copy`, `--copy`, or `--clipboard` to copy the share URL.".into()
    };
    Ok(format!(
        "Encrypted durable share bundle written to {}\nShare URL with decryption key: {}\n{}\nBackend: durable local file bundle. Move or sync the file anywhere you trust; the URL fragment/key is never written into the bundle.",
        file.display(),
        url,
        copy_status
    ))
}

pub(crate) fn handle_omfg(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let complaint = args.trim();
    if complaint.is_empty() {
        return Err("Usage: /omfg <complaint>".into());
    }
    let file = context.cwd.join(".jeden/rules.jsonl");
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let id = format!("rule-{}", now_text());
    let record = json!({
        "id": id,
        "kind": "omfg-rule",
        "createdAt": now_text(),
        "cwd": context.cwd,
        "complaint": complaint,
        "rule": format!("When this situation recurs, avoid the behavior described here: {}", complaint),
        "source": "/omfg"
    });
    let mut out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|e| e.to_string())?;
    writeln!(
        out,
        "{}",
        serde_json::to_string(&record).map_err(|e| e.to_string())?
    )
    .map_err(|e| e.to_string())?;
    Ok(format!(
        "Forged local rule {}.\nRules file: {}\nRule: {}",
        id,
        file.display(),
        record.get("rule").and_then(Value::as_str).unwrap_or("")
    ))
}

fn task_scheduler(
    context: &SlashContext<'_>,
) -> Result<crate::task_runtime::TaskScheduler, String> {
    let session_dir = slash_session_dir(context, "")?;
    crate::task_runtime::TaskScheduler::open(
        context.cwd,
        &session_dir.join("task-runtime"),
        crate::task_runtime::limits_from_config(context.cwd),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn handle_tan(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let task = args.trim();
    if task.is_empty() {
        return Err("Usage: /tan <work>".into());
    }
    let scheduler = task_scheduler(context)?;
    let job = scheduler
        .spawn(crate::task_runtime::SpawnRequest {
            task: task.into(),
            agent: "default".into(),
            model: context
                .model
                .filter(|model| !model.trim().is_empty())
                .map(str::to_string),
            max_steps: 6,
            parent_job: std::env::var("JEDEN_TASK_JOB")
                .ok()
                .filter(|value| !value.is_empty()),
            isolate: Some(true),
        })
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "Started durable task job {}.\nPID: {}\nWorkspace: {} ({})\nSession: {}\nUse /jobs to poll it.",
        job.id,
        job.pid.map(|pid| pid.to_string()).unwrap_or_else(|| "queued".into()),
        job.workspace.display(),
        job.isolation,
        job.session_path.display(),
    ))
}

fn tracked_jobs(context: &SlashContext<'_>) -> Option<(PathBuf, Vec<Value>)> {
    let scheduler = task_scheduler(context).ok()?;
    let dir = scheduler.store.join("jobs");
    let jobs = scheduler
        .list()
        .ok()?
        .into_iter()
        .filter_map(|job| serde_json::to_value(job).ok())
        .collect();
    Some((dir, jobs))
}

pub(super) fn jobs_picker(context: &SlashContext<'_>) -> PickerSpec {
    let Some((dir, jobs)) = tracked_jobs(context) else {
        return PickerSpec::new(
            "Background jobs",
            vec![
                PickerItem::action("No Rust session tracks background jobs", "")
                    .detail("Start a session and run `/tan <work>` manually.")
                    .badge("empty")
                    .disabled(true),
            ],
        );
    };
    let items = if jobs.is_empty() {
        vec![PickerItem::action("No tracked background jobs", "")
            .detail(format!("Job metadata directory: {}", dir.display()))
            .badge("empty")
            .disabled(true)]
    } else {
        jobs.into_iter()
            .map(|job| {
                let id = job.get("id").and_then(Value::as_str).unwrap_or("job");
                let recorded_status = job
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("not recorded");
                let task = job.get("task").and_then(Value::as_str).unwrap_or("");
                let pid = job
                    .get("pid")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "not recorded".into());
                PickerItem::action(id, format!("/copy {task}"))
                    .detail(format!(
                        "{task} — recorded PID {pid}; Enter copies the task"
                    ))
                    .badge(format!("recorded {recorded_status}"))
                    .disabled(task.is_empty())
            })
            .collect()
    };
    PickerSpec::new("Background jobs", items)
}

pub(crate) fn handle_jobs(context: &SlashContext<'_>) -> Result<String, String> {
    let Some((dir, jobs)) = tracked_jobs(context) else {
        return Ok("No background jobs are tracked for a Rust session yet.".into());
    };
    if jobs.is_empty() {
        Ok(format!(
            "No background jobs are tracked in {}.",
            dir.display()
        ))
    } else {
        serde_json::to_string_pretty(&jobs).map_err(|e| e.to_string())
    }
}
