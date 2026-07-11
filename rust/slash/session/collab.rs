use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use url::Url;

use crate::slash::common::{file_url, now_text, read_json_value, split_head, write_json_value};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

fn collab_state_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/collab.json")
}

fn collab_default_relay(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/collab-relay.jsonl")
}

fn collab_path(cwd: &Path, target: &str) -> Result<PathBuf, String> {
    let text = target.trim();
    if text.starts_with("http://") || text.starts_with("https://") {
        return Err("Rust collab currently supports durable file relays only; HTTP relay support remains JS-only.".into());
    }
    if text.starts_with("file://") {
        let url = Url::parse(text).map_err(|e| e.to_string())?;
        return url
            .to_file_path()
            .map_err(|_| "Invalid file relay URL".to_string());
    }
    if text.is_empty() {
        return Ok(collab_default_relay(cwd));
    }
    let path = PathBuf::from(text);
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

fn append_collab_event(path: &Path, event_type: &str, cwd: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let line = serde_json::to_string(&json!({ "ts": now_text(), "type": event_type, "cwd": cwd }))
        .map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{}", line).map_err(|e| e.to_string())
}

fn read_collab_events(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn collab_descriptor(entry: &Value) -> String {
    if let Some(file) = entry.get("relayFile").and_then(Value::as_str) {
        format!("durable file relay: {}", file)
    } else {
        "off".into()
    }
}

fn collab_role_status(role: &str, entry: &Value, view: bool) -> String {
    if entry.is_null() {
        return format!("Collab {role}: off.");
    }
    let relay_file = entry.get("relayFile").and_then(Value::as_str).unwrap_or("");
    let events = read_collab_events(Path::new(relay_file));
    let latest = events
        .last()
        .and_then(|event| event.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("none");
    let mut lines = vec![
        format!("Collab {role}: {}", collab_descriptor(entry)),
        format!(
            "Relay URL: {}",
            entry.get("relayUrl").and_then(Value::as_str).unwrap_or("")
        ),
        format!("Events: {}", events.len()),
        format!("Latest event: {}", latest),
    ];
    if view {
        if events.is_empty() {
            lines.push("Event log is empty.".into());
        } else {
            lines.push("Event log:".into());
            // A number-free running ordinal: start from the u64 default and step
            // by the unit derived from `true`. `Value` renders via `Display`
            // (infallible), so no serialize recovery path is needed.
            let mut ordinal = u64::default();
            for event in &events {
                ordinal += u64::from(true);
                lines.push(format!("{}. {}", ordinal, event));
            }
        }
    }
    lines.join("\n")
}

fn save_collab_state(cwd: &Path, state: &Value) -> Result<PathBuf, String> {
    let file = collab_state_path(cwd);
    let host = state.get("host").cloned().unwrap_or(Value::Null);
    let guest = state.get("guest").cloned().unwrap_or(Value::Null);
    write_json_value(
        &file,
        &json!({ "updatedAt": now_text(), "host": host, "guest": guest }),
    )?;
    Ok(file)
}

/// Encrypt a collab event under `key` and POST it to the HTTP relay. The relay
/// only ever sees the ciphertext; the key never leaves this process. The key is
/// taken as a slice and converted to the fixed-size array the cipher requires,
/// so no array-length literal is written here.
fn post_collab_http(
    base: &str,
    room: &str,
    key: &[u8],
    write_token: &str,
    event_type: &str,
    cwd: &Path,
) -> Result<(), String> {
    let key_array: &[u8; 32] = key
        .try_into()
        .map_err(|_| "collab key has an unexpected length".to_string())?;
    let frame = crate::collab::ProtocolFrame::new(
        "jeden-slash",
        crate::collab::CollabRole::Full,
        crate::collab::FrameKind::State {
            value: json!({ "event": event_type, "ts": now_text(), "cwd": cwd }),
        },
    )?;
    let blob = crate::collab::seal_frame(key_array, &frame)?;
    crate::collab::relay_post_authorized(base, room, &blob, Some(write_token))?;
    Ok(())
}

/// Status for an HTTP-backed collab role. Shows the relay base + room + live
/// event count fetched from the relay (opaque blobs; contents stay encrypted).
fn collab_http_role_status(role: &str, entry: &Value) -> String {
    let base = entry.get("relayBase").and_then(Value::as_str).unwrap_or("");
    let room = entry.get("room").and_then(Value::as_str).unwrap_or("");
    let count =
        crate::collab::relay_get(base, room, usize::default()).map(|(events, _)| events.len());
    let events_line = match count {
        Ok(n) => format!("Events: {} (encrypted)", n),
        Err(e) => format!("Events: unavailable ({})", e),
    };
    [
        format!("Collab {role}: HTTP relay {}", base),
        format!("Room: {}", room),
        events_line,
        "Payloads are end-to-end encrypted; the relay never sees plaintext or the key.".to_string(),
    ]
    .join("\n")
}

fn picker_role_detail(role: &str, entry: &Value) -> String {
    if entry.is_null() {
        return format!("{role}: off");
    }
    match entry.get("backend").and_then(Value::as_str) {
        Some("http") => format!(
            "{role}: HTTP relay {} room {}; encryption key is not persisted",
            entry
                .get("relayBase")
                .and_then(Value::as_str)
                .unwrap_or("not recorded"),
            entry
                .get("room")
                .and_then(Value::as_str)
                .unwrap_or("not recorded")
        ),
        _ => format!("{role}: {}", collab_descriptor(entry)),
    }
}

pub(super) fn build_collab_picker(context: &SlashContext<'_>) -> PickerSpec {
    let state = read_json_value(&collab_state_path(context.cwd));
    let host = state.get("host").unwrap_or(&Value::Null);
    let guest = state.get("guest").unwrap_or(&Value::Null);
    let detail = format!(
        "{}; {}",
        picker_role_detail("host", host),
        picker_role_detail("guest", guest)
    );
    PickerSpec::new(
        "Collaboration",
        vec![
            PickerItem::action("Show collaboration status", "/collab status")
                .detail(&detail)
                .badge("status"),
            PickerItem::action("View collaboration events", "/collab view")
                .detail(&detail)
                .badge("event log"),
            PickerItem::action("Start host on default durable relay", "/collab start")
                .detail(format!(
                    "Default relay: {}",
                    collab_default_relay(context.cwd).display()
                ))
                .badge("writes relay")
                .disabled(!host.is_null()),
            PickerItem::action("Stop active host", "/collab stop")
                .detail(picker_role_detail("host", host))
                .badge("destructive")
                .disabled(host.is_null()),
        ],
    )
}

pub(super) fn build_join_picker(context: &SlashContext<'_>) -> PickerSpec {
    let state = read_json_value(&collab_state_path(context.cwd));
    let host = state.get("host").unwrap_or(&Value::Null);
    let guest = state.get("guest").unwrap_or(&Value::Null);
    let mut items = Vec::new();
    if let Some(relay_url) = host.get("relayUrl").and_then(Value::as_str) {
        items.push(
            PickerItem::action(
                "Join the active local host relay",
                format!("/join {relay_url}"),
            )
            .detail(picker_role_detail("host", host))
            .badge("durable file")
            .disabled(!guest.is_null()),
        );
    }
    let instruction = if host.get("backend").and_then(Value::as_str) == Some("http") {
        "The HTTP join key is intentionally not persisted; paste the private join URL as `/join <url>#key=<key>`."
    } else {
        "A relay target is required; type `/join <relay-file-or-file-url>` manually."
    };
    items.push(
        PickerItem::action("Enter another relay target", "/join ")
            .detail(instruction)
            .badge("INPUT")
            .prefill(),
    );
    if !guest.is_null() {
        items.push(
            PickerItem::action("Already attached as guest", "")
                .detail(picker_role_detail("guest", guest))
                .badge("current")
                .disabled(true),
        );
    }
    PickerSpec::new("Join collaboration", items)
}

pub(super) fn build_leave_picker(context: &SlashContext<'_>) -> PickerSpec {
    let state = read_json_value(&collab_state_path(context.cwd));
    let guest = state.get("guest").unwrap_or(&Value::Null);
    PickerSpec::new(
        "Leave collaboration",
        vec![
            PickerItem::action("Leave active guest relay", "/leave confirmed")
                .detail(picker_role_detail("guest", guest))
                .badge("destructive")
                .disabled(guest.is_null()),
        ],
    )
}

pub(crate) fn handle_collab(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    let verb = if verb.is_empty() { "status" } else { verb };
    let mut state = read_json_value(&collab_state_path(context.cwd));
    if !state.is_object() {
        state = json!({});
    }
    if verb == "status" || verb == "view" {
        let host = state.get("host").unwrap_or(&Value::Null);
        let guest = state.get("guest").unwrap_or(&Value::Null);
        let file = collab_state_path(context.cwd);
        if host.is_null() && guest.is_null() {
            return Ok(format!("Collab off.\nRust backend: durable local file relay in .jeden/collab-relay.jsonl.\nState: {}", file.display()));
        }
        let mut sections = Vec::new();
        if !host.is_null() {
            sections.push(
                if host.get("backend").and_then(Value::as_str) == Some("http") {
                    collab_http_role_status("host", host)
                } else {
                    collab_role_status("host", host, verb == "view")
                },
            );
        }
        if !guest.is_null() {
            sections.push(
                if guest.get("backend").and_then(Value::as_str) == Some("http") {
                    collab_http_role_status("guest", guest)
                } else {
                    collab_role_status("guest", guest, verb == "view")
                },
            );
        }
        sections.push(format!("State: {}", file.display()));
        return Ok(sections.join("\n\n"));
    }
    if verb == "start" {
        let target = rest.trim();
        if target.starts_with("http://") || target.starts_with("https://") {
            let parsed = crate::collab::parse_relay_url(target)?;
            let (room, key) = if parsed.room.is_empty() {
                crate::collab::new_room_and_key()
            } else {
                (
                    parsed.room.clone(),
                    parsed
                        .key
                        .ok_or("HTTP relay start URL with a room must include #key=<k>")?,
                )
            };
            let write_token = parsed
                .write_token
                .unwrap_or_else(crate::collab::new_write_token);
            post_collab_http(
                &parsed.base,
                &room,
                &key,
                &write_token,
                "host-start",
                context.cwd,
            )?;
            let key_text = crate::collab::encode_key(&key);
            let entry = json!({ "backend": "http", "relayBase": parsed.base, "room": room, "key": key_text, "writeToken": write_token, "cursor": 1, "role": "full", "startedAt": now_text(), "cwd": context.cwd });
            state["host"] = entry;
            save_collab_state(context.cwd, &state)?;
            let view_url = format!("{}/room/{}#key={}&role=view", parsed.base, room, key_text);
            let full_url = format!(
                "{}/room/{}#key={}&write={}&role=full",
                parsed.base, room, key_text, write_token
            );
            return Ok(format!(
                "Collab started on durable E2EE relay {}.\nView URL: {}\nFull write URL: {}\nThe encryption key and separately revocable write token stay in URL fragments and are never sent by browser navigation.",
                parsed.base, view_url, full_url
            ));
        }
        let relay = collab_path(context.cwd, rest)?;
        append_collab_event(&relay, "host-start", context.cwd)?;
        let entry = json!({ "backend": "file", "relayFile": relay, "relayUrl": file_url(&relay), "startedAt": now_text(), "cwd": context.cwd });
        state["host"] = entry;
        let file = save_collab_state(context.cwd, &state)?;
        return Ok(format!("Collab started with durable file relay: {}.\nJoin with: /join {}\nBackend: durable local file relay.\nState: {}", relay.display(), file_url(&relay), file.display()));
    }
    if verb == "stop" {
        let host = state.get("host").cloned().unwrap_or(Value::Null);
        if host.is_null() {
            return Ok("Collab hosting is already stopped.".into());
        }
        if let Some(relay_file) = host.get("relayFile").and_then(Value::as_str) {
            append_collab_event(Path::new(relay_file), "host-stop", context.cwd)?;
        }
        state["host"] = Value::Null;
        let file = save_collab_state(context.cwd, &state)?;
        return Ok(format!(
            "Collab hosting stopped.\nState: {}",
            file.display()
        ));
    }
    Err("Usage: /collab [start|status|view|stop] [relay-file | http://relay-host[:port]]".into())
}

pub(crate) fn handle_join(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let target = args.trim();
    if target.is_empty() {
        return Err("Usage: /join <http-relay-url | relay-file>".into());
    }
    if target.starts_with("http://") || target.starts_with("https://") {
        let parsed = crate::collab::parse_relay_url(target)?;
        if parsed.room.is_empty() {
            return Err("Join URL must include /room/<id>#key=<k>".into());
        }
        let key = parsed
            .key
            .ok_or("Join URL must include the #key=<k> fragment")?;
        let (blobs, cursor) = crate::collab::relay_get(&parsed.base, &parsed.room, 0)?;
        if blobs.is_empty() {
            return Err("No events in that relay room yet — check the room id.".into());
        }
        let mut frames = Vec::with_capacity(blobs.len());
        for blob in &blobs {
            frames.push(
                crate::collab::open_frame(&key, blob)
                    .map_err(|e| format!("relay payload failed to decrypt: {e}"))?,
            );
        }
        if let Some(token) = parsed.write_token.as_deref() {
            post_collab_http(
                &parsed.base,
                &parsed.room,
                &key,
                token,
                "guest-join",
                context.cwd,
            )?;
        }
        let mut state = read_json_value(&collab_state_path(context.cwd));
        if !state.is_object() {
            state = json!({});
        }
        let role = match parsed.role {
            crate::collab::CollabRole::View => "view",
            crate::collab::CollabRole::Prompt => "prompt",
            crate::collab::CollabRole::Abort => "abort",
            crate::collab::CollabRole::Full => "full",
        };
        state["guest"] = json!({ "backend": "http", "relayBase": parsed.base, "room": parsed.room, "key": crate::collab::encode_key(&key), "writeToken": parsed.write_token, "role": role, "cursor": cursor, "joinedAt": now_text(), "cwd": context.cwd });
        save_collab_state(context.cwd, &state)?;
        return Ok(format!(
            "Joined HTTP collab relay {} room {} as {}. Replayed {} encrypted frame(s); cursor {}.",
            parsed.base,
            parsed.room,
            role,
            frames.len(),
            cursor
        ));
    }
    let relay = collab_path(context.cwd, target)?;
    append_collab_event(&relay, "guest-join", context.cwd)?;
    let mut state = read_json_value(&collab_state_path(context.cwd));
    if !state.is_object() {
        state = json!({});
    }
    state["guest"] = json!({ "backend": "file", "relayFile": relay, "relayUrl": file_url(&relay), "joinedAt": now_text(), "cwd": context.cwd });
    let file = save_collab_state(context.cwd, &state)?;
    Ok(format!(
        "Joined collab via durable file relay: {}.\nRelay URL: {}\nState: {}",
        relay.display(),
        file_url(&relay),
        file.display()
    ))
}

pub(crate) fn handle_leave(context: &SlashContext<'_>) -> Result<String, String> {
    let mut state = read_json_value(&collab_state_path(context.cwd));
    if !state.is_object() {
        state = json!({});
    }
    let guest = state.get("guest").cloned().unwrap_or(Value::Null);
    if guest.is_null() {
        let host_note = if !state.get("host").unwrap_or(&Value::Null).is_null() {
            " Hosting is still active; use /collab stop to stop the host relay."
        } else {
            ""
        };
        return Ok(format!(
            "No guest collab attachment is active.{}",
            host_note
        ));
    }
    if let Some(relay_file) = guest.get("relayFile").and_then(Value::as_str) {
        append_collab_event(Path::new(relay_file), "guest-leave", context.cwd)?;
    }
    state["guest"] = Value::Null;
    let file = save_collab_state(context.cwd, &state)?;
    Ok(format!("Left collab relay.\nState: {}", file.display()))
}
