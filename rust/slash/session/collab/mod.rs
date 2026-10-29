//! Two people working in one session: starting a relay, joining one, and
//! leaving it.
//!
//! Every command reads the saved collaboration state first, through
//! `read_collab_state`: a state file that cannot be read or does not parse is
//! refused naming the file, before any relay event is written, rather than
//! read as "no session" and then overwritten by the next save. A role the
//! state does not carry indexes as `null`.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::slash::common::{file_url, now_text, split_head};
use crate::slash::SlashContext;

mod pickers;
mod relay;
mod status;

pub(crate) use pickers::{build_collab_picker, build_join_picker, build_leave_picker};
use relay::{
    append_collab_event, collab_path, collab_state_path, is_network_relay, on_http_relay,
    post_collab_http, read_collab_state, save_collab_state,
};
use status::{collab_http_role_status, collab_role_status};
use std::path::Path;

const COLLAB_USAGE: &str =
    "Usage: /collab [start|status|view|stop] [relay-file | http://relay-host[:port]]";

/// The `/collab` verbs; none given is `status`.
#[derive(Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
enum CollabVerb {
    Status,
    View,
    Start,
    Stop,
}

fn collab_verb(verb: &str) -> Result<CollabVerb, String> {
    if verb.is_empty() {
        return Ok(CollabVerb::Status);
    }
    serde_json::from_value(Value::String(verb.to_string()))
        .map_err(|error| format!("unknown /collab verb `{verb}` ({error}). {COLLAB_USAGE}"))
}

pub(crate) fn handle_collab(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    let verb = collab_verb(verb)?;
    let mut state = read_collab_state(context.cwd)?;
    match verb {
        CollabVerb::Status | CollabVerb::View => {
            Ok(collab_status(&state, context.cwd, verb == CollabVerb::View))
        }
        CollabVerb::Start => collab_start(&mut state, rest, context),
        CollabVerb::Stop => {
            let host = state["host"].clone();
            if host.is_null() {
                return Ok("Collab hosting is already stopped.".into());
            }
            if let Some(relay_file) = host.get("relayFile").and_then(Value::as_str) {
                append_collab_event(Path::new(relay_file), "host-stop", context.cwd)?;
            }
            state["host"] = Value::Null;
            let file = save_collab_state(context.cwd, &state)?;
            Ok(format!(
                "Collab hosting stopped.\nState: {}",
                file.display()
            ))
        }
    }
}

fn collab_status(state: &Value, cwd: &Path, view: bool) -> String {
    let host = &state["host"];
    let guest = &state["guest"];
    let file = collab_state_path(cwd);
    if host.is_null() && guest.is_null() {
        return format!("Collab off.\nRust backend: durable local file relay in .jeden/collab-relay.jsonl.\nState: {}", file.display());
    }
    let mut sections = Vec::new();
    for (role, entry) in [("host", host), ("guest", guest)] {
        if entry.is_null() {
            continue;
        }
        sections.push(if on_http_relay(entry) {
            collab_http_role_status(role, entry)
        } else {
            collab_role_status(role, entry, view)
        });
    }
    sections.push(format!("State: {}", file.display()));
    sections.join("\n\n")
}

fn collab_start(
    state: &mut Value,
    rest: &str,
    context: &SlashContext<'_>,
) -> Result<String, String> {
    // Read before any relay is touched, so a clock that cannot be read
    // starts nothing.
    let started = now_text()?;
    let target = rest.trim();
    if is_network_relay(target) {
        let parsed = crate::collab::parse_relay_url(target)?;
        let (room, key) = match parsed.room {
            None => (crate::collab::new_room(), crate::collab::new_key()),
            Some(room) => (
                room,
                parsed
                    .key
                    .ok_or("HTTP relay start URL with a room must include #key=<k>")?,
            ),
        };
        let write_token = match parsed.write_token {
            Some(token) => token,
            None => crate::collab::new_write_token(),
        };
        post_collab_http(
            &parsed.base,
            &room,
            &key,
            &write_token,
            "host-start",
            context.cwd,
        )?;
        // The cursor is where the relay says this room's log now ends, the
        // host's own start event included.
        let (_, cursor) = crate::collab::relay_get(&parsed.base, &room)?;
        let key_text = crate::collab::encode_key(&key);
        let (full, view) = (
            crate::collab::CollabRole::Full.name(),
            crate::collab::CollabRole::View.name(),
        );
        let entry = json!({ "backend": "http", "relayBase": parsed.base, "room": room, "key": key_text, "writeToken": write_token, "cursor": cursor, "role": full, "startedAt": started, "cwd": context.cwd });
        state["host"] = entry;
        save_collab_state(context.cwd, state)?;
        let view_url = format!("{}/room/{}#key={}&role={view}", parsed.base, room, key_text);
        let full_url = format!(
            "{}/room/{}#key={}&write={}&role={full}",
            parsed.base, room, key_text, write_token
        );
        let qr_block = |url: &str| match crate::qr::render(url) {
            Ok(qr) => format!("\n{qr}"),
            Err(cause) => format!("\n(No QR code for this link: {cause}; open it as text.)"),
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
    let entry = json!({ "backend": "file", "relayFile": relay, "relayUrl": file_url(&relay), "startedAt": started, "cwd": context.cwd });
    state["host"] = entry;
    let file = save_collab_state(context.cwd, state)?;
    Ok(format!("Collab started with durable file relay: {}.\nJoin with: /join {}\nBackend: durable local file relay.\nState: {}", relay.display(), file_url(&relay), file.display()))
}

pub(crate) fn handle_join(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let target = args.trim();
    if target.is_empty() {
        return Err("Usage: /join <http-relay-url | relay-file>".into());
    }
    let mut state = read_collab_state(context.cwd)?;
    let joined = now_text()?;
    if is_network_relay(target) {
        let parsed = crate::collab::parse_relay_url(target)?;
        let room = parsed
            .room
            .ok_or("Join URL must include /room/<id>#key=<k>")?;
        let key = parsed
            .key
            .ok_or("Join URL must include the #key=<k> fragment")?;
        let (blobs, cursor) = crate::collab::relay_get(&parsed.base, &room)?;
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
            post_collab_http(&parsed.base, &room, &key, token, "guest-join", context.cwd)?;
        }
        let role = parsed.role.name();
        state["guest"] = json!({ "backend": "http", "relayBase": parsed.base, "room": room, "key": crate::collab::encode_key(&key), "writeToken": parsed.write_token, "role": role, "cursor": cursor, "joinedAt": joined, "cwd": context.cwd });
        save_collab_state(context.cwd, &state)?;
        return Ok(format!(
            "Joined HTTP collab relay {} room {} as {}. Replayed {} encrypted frame(s); cursor {}.",
            parsed.base,
            room,
            role,
            frames.len(),
            json!(cursor)
        ));
    }
    let relay = collab_path(context.cwd, target)?;
    append_collab_event(&relay, "guest-join", context.cwd)?;
    state["guest"] = json!({ "backend": "file", "relayFile": relay, "relayUrl": file_url(&relay), "joinedAt": joined, "cwd": context.cwd });
    let file = save_collab_state(context.cwd, &state)?;
    Ok(format!(
        "Joined collab via durable file relay: {}.\nRelay URL: {}\nState: {}",
        relay.display(),
        file_url(&relay),
        file.display()
    ))
}

pub(crate) fn handle_leave(context: &SlashContext<'_>) -> Result<String, String> {
    let mut state = read_collab_state(context.cwd)?;
    let guest = state["guest"].clone();
    if guest.is_null() {
        let host_note = if !state["host"].is_null() {
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
