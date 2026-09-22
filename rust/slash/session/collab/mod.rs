//! Two people working in one session: starting a relay, joining one, and
//! leaving it.

use serde_json::{json, Value};

use crate::slash::common::{file_url, now_text, read_json_value, split_head};
use crate::slash::SlashContext;

mod pickers;
mod relay;
mod status;

pub(crate) use pickers::{
    build_collab_picker, build_join_picker, build_leave_picker,
};
use relay::{append_collab_event, collab_path, collab_state_path, post_collab_http, save_collab_state};
use status::{collab_http_role_status, collab_role_status};
use std::path::Path;

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
            let qr_block = |url: &str| {
                crate::qr::render(url)
                    .map(|qr| format!("\n{qr}"))
                    .unwrap_or_default()
            };
            return Ok(format!(
                "Collab started on durable E2EE relay {}.\nView URL: {}{}\nFull write URL: {}{}\nThe encryption key and separately revocable write token stay in URL fragments and are never sent by browser navigation.",
                parsed.base,
                view_url,
                qr_block(&view_url),
                full_url,
                qr_block(&full_url)
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
