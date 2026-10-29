//! Publishing a session where somebody else can read it, and forging a local
//! rule from a complaint the operator just typed.
//!
//! Split out of `slash/session/mod.rs`, which had grown past the module line
//! cap.

use crate::slash::common::{file_url, now_text, split_args};
use crate::slash::plugins::marketplace::sanitize_marketplace_name;
use crate::slash::session::clipboard::write_clipboard;
use crate::slash::session::{slash_session_dir, slash_session_value};
use crate::slash::SlashContext;
use serde_json::json;
use serde_json::Value;
use std::fs;
use std::io::Write;

pub(crate) fn handle_share(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let copy_link = match split_args(args).as_slice() {
        [] => false,
        [flag] if flag == "--copy" => true,
        other => {
            return Err(format!(
                "/share takes only --copy (to copy the share URL), not {}",
                other.join(" ")
            ))
        }
    };
    let session = slash_session_value(context, "")?;
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or("this session has no id yet; send a message first so it is recorded")?;
    let created_at = now_text()?;
    let plain = serde_json::to_vec_pretty(
        &json!({ "kind": "jeden-session", "createdAt": created_at, "session": session }),
    )
    .map_err(|error| format!("session {id} cannot be encoded for sharing: {error}"))?;
    // The framed blob carries the nonce, ciphertext and tag together; the key
    // is returned only in the URL fragment and is never written into the bundle.
    let key = crate::collab::new_key();
    let blob = crate::collab::encrypt_blob(&key, &plain)?;
    let session_dir = slash_session_dir(context, "")?;
    let artifact_dir = session_dir.join("artifacts");
    fs::create_dir_all(&artifact_dir)
        .map_err(|error| format!("{} cannot be created: {error}", artifact_dir.display()))?;
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
    let encoded = serde_json::to_string_pretty(&bundle)
        .map_err(|error| format!("the share bundle cannot be encoded: {error}"))?;
    fs::write(&file, encoded + "\n")
        .map_err(|error| format!("{} cannot be written: {error}", file.display()))?;
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
        "Add `--copy` to copy the share URL.".into()
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
        fs::create_dir_all(parent)
            .map_err(|error| format!("{} cannot be created: {error}", parent.display()))?;
    }
    let created_at = now_text()?;
    let id = format!("rule-{created_at}");
    let rule =
        format!("When this situation recurs, avoid the behavior described here: {complaint}");
    let record = json!({
        "id": id,
        "kind": "omfg-rule",
        "createdAt": created_at,
        "cwd": context.cwd,
        "complaint": complaint,
        "rule": rule,
        "source": "/omfg"
    });
    let line = serde_json::to_string(&record)
        .map_err(|error| format!("rule {id} cannot be encoded: {error}"))?;
    let mut out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|error| format!("{} cannot be opened for appending: {error}", file.display()))?;
    writeln!(out, "{line}").map_err(|error| {
        format!(
            "rule {id} cannot be appended to {}: {error}",
            file.display()
        )
    })?;
    Ok(format!(
        "Forged local rule {}.\nRules file: {}\nRule: {}",
        id,
        file.display(),
        rule
    ))
}
