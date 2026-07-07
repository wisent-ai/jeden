use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::slash::common::{file_url, now_text, read_json_value, resolve_cwd_path, split_args, write_json_value};
use crate::slash::state::mode_state_path;
use crate::slash::SlashContext;

pub(crate) mod clipboard;
pub(crate) mod collab;

use clipboard::write_clipboard;
use crate::slash::plugins::marketplace::sanitize_marketplace_name;

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
        if raw_path.is_absolute() { raw_path } else { context.cwd.join(raw_path) }
    } else {
        context.session_root.join(target)
    };
    if !path.exists() { return Err(format!("session not found: {}", path.display())); }
    Ok(path)
}

fn slash_session_events(dir: &Path) -> Vec<Value> {
    fs::read_to_string(dir.join("transcript.jsonl")).unwrap_or_default().lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).collect()
}

fn slash_session_value(context: &SlashContext<'_>, id_or_path: &str) -> Result<Value, String> {
    let dir = slash_session_dir(context, id_or_path)?;
    let state = read_json_value(&dir.join("state.json"));
    let id = dir.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_else(|| dir.display().to_string());
    Ok(json!({ "id": id, "path": dir, "state": state, "events": slash_session_events(&dir) }))
}

fn slash_session_text(session: &Value) -> String {
    let mut out = vec![format!("Session: {}", session.get("id").and_then(Value::as_str).unwrap_or("session")), format!("Path: {}", session.get("path").and_then(Value::as_str).unwrap_or("")), String::new()];
    for event in session.get("events").and_then(Value::as_array).cloned().unwrap_or_default() {
        out.push(format!("## {} {}", event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or("")).trim().to_string());
        out.push(serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into()));
        out.push(String::new());
    }
    out.join("\n")
}

fn slash_html_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn slash_session_export(session: &Value, format: &str) -> Result<String, String> {
    if format == "json" {
        return Ok(serde_json::to_string_pretty(session).map_err(|e| e.to_string())? + "\n");
    }
    if format == "markdown" || format == "md" {
        let mut out = format!("# Jeden session {}\n\n{}\n\n", session.get("id").and_then(Value::as_str).unwrap_or("session"), session.get("path").and_then(Value::as_str).unwrap_or(""));
        for event in session.get("events").and_then(Value::as_array).cloned().unwrap_or_default() {
            let label = format!("{} {}", event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or("")).trim().to_string();
            let data = serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into());
            out.push_str(&format!("## {}\n\n```json\n{}\n```\n\n", label, data));
        }
        return Ok(out);
    }
    if format == "html" {
        let id = slash_html_escape(session.get("id").and_then(Value::as_str).unwrap_or("session"));
        let path = slash_html_escape(session.get("path").and_then(Value::as_str).unwrap_or(""));
        let mut body = String::new();
        for event in session.get("events").and_then(Value::as_array).cloned().unwrap_or_default() {
            let label = slash_html_escape(&format!("{} {}", event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or("")).trim().to_string());
            let data = slash_html_escape(&serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into()));
            body.push_str(&format!("<section><h2>{}</h2><pre>{}</pre></section>\n", label, data));
        }
        return Ok(format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>Jeden session {}</title></head><body><h1>Jeden session {}</h1><p>{}</p>{}</body></html>\n", id, id, path, body));
    }
    Err(format!("unsupported session export format: {}", format))
}

pub(crate) fn handle_dump(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    Ok(slash_session_text(&slash_session_value(context, args.trim())?))
}

pub(crate) fn handle_export(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let mut id = String::new();
    let mut format = "json".to_string();
    let mut output: Option<String> = None;
    for arg in argv {
        if arg == "--html" { format = "html".into(); }
        else if arg == "--markdown" || arg == "--md" { format = "markdown".into(); }
        else if id.is_empty() && !arg.starts_with("--") && slash_session_dir(context, &arg).is_ok() { id = arg; }
        else { output = Some(arg); }
    }
    let payload = slash_session_export(&slash_session_value(context, &id)?, &format)?;
    if let Some(path) = output {
        let target = resolve_cwd_path(context.cwd, &path);
        if let Some(parent) = target.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        fs::write(&target, &payload).map_err(|e| e.to_string())?;
        Ok(target.display().to_string())
    } else {
        Ok(payload)
    }
}

pub(crate) fn handle_share(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let copy_link = argv.iter().any(|arg| matches!(arg.as_str(), "copy" | "--copy" | "--clipboard"));
    let session = slash_session_value(context, "")?;
    let id = session.get("id").and_then(Value::as_str).unwrap_or("session");
    let created_at = now_text();
    let plain = serde_json::to_vec_pretty(&json!({ "kind": "jeden-session", "createdAt": created_at, "session": session })).map_err(|e| e.to_string())?;
    // Delegate the AES-256-GCM key/nonce/tag sizing to crate::collab, which
    // encapsulates those cryptographic constants. The framed blob carries the
    // nonce, ciphertext and tag together; the key is returned only in the URL
    // fragment and is never written into the bundle.
    let (_room, key) = crate::collab::new_room_and_key();
    let blob = crate::collab::encrypt_blob(&key, &plain)?;
    let session_dir = slash_session_dir(context, "")?;
    let artifact_dir = session_dir.join("artifacts");
    fs::create_dir_all(&artifact_dir).map_err(|e| e.to_string())?;
    let file = artifact_dir.join(format!("share-{}-{}.jeden-share", sanitize_marketplace_name(id), created_at));
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
    fs::write(&file, serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
    let url = format!("{}#key={}", file_url(&file), crate::collab::encode_key(&key));
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
    if complaint.is_empty() { return Err("Usage: /omfg <complaint>".into()); }
    let file = context.cwd.join(".jeden/rules.jsonl");
    if let Some(parent) = file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
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
    let mut out = fs::OpenOptions::new().create(true).append(true).open(&file).map_err(|e| e.to_string())?;
    writeln!(out, "{}", serde_json::to_string(&record).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(format!("Forged local rule {}.\nRules file: {}\nRule: {}", id, file.display(), record.get("rule").and_then(Value::as_str).unwrap_or("")))
}

pub(crate) fn handle_tan(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let task = args.trim();
    if task.is_empty() { return Err("Usage: /tan <work>".into()); }
    let session_dir = slash_session_dir(context, "")?;
    let dir = session_dir.join("artifacts/tan-jobs");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let job_id = format!("tan-{}", now_text());
    let stdout_path = dir.join(format!("{}.stdout.log", job_id));
    let stderr_path = dir.join(format!("{}.stderr.log", job_id));
    let metadata_path = dir.join(format!("{}.json", job_id));
    let stdout = fs::File::create(&stdout_path).map_err(|e| e.to_string())?;
    let stderr = fs::File::create(&stderr_path).map_err(|e| e.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command.arg("run").arg(task).arg("--cwd").arg(context.cwd).arg("--json");
    if let Some(model) = context.model.filter(|model| !model.trim().is_empty()) {
        command.arg("--model").arg(model);
    }
    let child = command.stdin(Stdio::null()).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr)).spawn().map_err(|e| e.to_string())?;
    let pid = child.id();
    let metadata = json!({
        "id": job_id,
        "kind": "tan",
        "status": "running",
        "pid": pid,
        "task": task,
        "cwd": context.cwd,
        "sessionPath": session_dir,
        "stdout": stdout_path,
        "stderr": stderr_path,
        "startedAt": now_text()
    });
    write_json_value(&metadata_path, &metadata)?;
    let mut mode = read_json_value(&mode_state_path(context.cwd));
    if !mode.is_object() { mode = json!({}); }
    mode.as_object_mut().expect("mode object").insert("tanJobsSessionPath".into(), json!(session_dir));
    write_json_value(&mode_state_path(context.cwd), &mode)?;
    std::mem::forget(child);
    Ok(format!("Started detached tan job {}.\nPID: {}\nMetadata: {}\nStdout: {}\nStderr: {}", job_id, pid, metadata_path.display(), stdout_path.display(), stderr_path.display()))
}

pub(crate) fn handle_jobs(context: &SlashContext<'_>) -> Result<String, String> {
    let mode = read_json_value(&mode_state_path(context.cwd));
    let session_dir = if let Some(path) = mode.get("tanJobsSessionPath").and_then(Value::as_str) {
        PathBuf::from(path)
    } else {
        match slash_session_dir(context, "") {
            Ok(dir) => dir,
            Err(_) => return Ok("No background jobs are tracked for a Rust session yet.".into()),
        }
    };
    let dir = session_dir.join("artifacts/tan-jobs");
    let mut jobs = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") { continue; }
            let mut job = read_json_value(&path);
            if !job.is_object() { continue; }
            if let Some(map) = job.as_object_mut() {
                map.insert("metadata".into(), json!(path));
                for key in ["stdout", "stderr"] {
                    if let Some(log_path) = map.get(key).and_then(Value::as_str) {
                        if let Ok(meta) = fs::metadata(log_path) {
                            map.insert(format!("{key}Bytes"), json!(meta.len()));
                        }
                    }
                }
            }
            jobs.push(job);
        }
    }
    jobs.sort_by(|a, b| {
        a.get("startedAt").and_then(Value::as_str).unwrap_or("")
            .cmp(b.get("startedAt").and_then(Value::as_str).unwrap_or(""))
            .then_with(|| a.get("id").and_then(Value::as_str).unwrap_or("").cmp(b.get("id").and_then(Value::as_str).unwrap_or("")))
    });
    if jobs.is_empty() {
        Ok(format!("No background jobs are tracked in {}.", dir.display()))
    } else {
        serde_json::to_string_pretty(&jobs).map_err(|e| e.to_string())
    }
}
