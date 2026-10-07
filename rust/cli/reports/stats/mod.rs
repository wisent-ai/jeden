//! `jeden stats` — local usage/quota/session snapshot as text, `--json`, or a
//! self-contained local web dashboard (`--serve [--port N]`). Without
//! `--port` the operating system assigns a free port and the command prints
//! the address it bound. The dashboard binds 127.0.0.1 only, serves a single
//! HTML page plus a `/api/stats` JSON endpoint, with explicit snapshot refresh.

mod dashboard;

use dashboard::serve;
use serde_json::{json, Value};
use std::path::Path;

use crate::control_plane::now_ms;
use crate::control_plane::quota::{
    fetch_subscription_quotas, percent_free, QuotaEntry, SubscriptionQuotas,
};
use crate::read_json;
use crate::Args;

fn usage_file_totals(path: &Path) -> Value {
    let usage = read_json::<Value>(path);
    let events: Vec<Value> = usage
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut calls = 0_u64;
    let (mut tokens, mut cost) = (0_f64, 0_f64);
    let mut by_model = serde_json::Map::new();
    for event in &events {
        calls += 1;
        let event_tokens = event
            .get("totalTokens")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                [
                    "inputTokens",
                    "outputTokens",
                    "cacheReadTokens",
                    "cacheWriteTokens",
                ]
                .iter()
                .map(|key| event.get(key).and_then(Value::as_f64).unwrap_or_default())
                .sum::<f64>()
            });
        let event_cost = event
            .pointer("/cost/total")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                ["input", "output", "cacheRead", "cacheWrite"]
                    .iter()
                    .map(|key| {
                        event
                            .pointer(&format!("/cost/{key}"))
                            .and_then(Value::as_f64)
                            .unwrap_or_default()
                    })
                    .sum::<f64>()
            });
        tokens += event_tokens;
        cost += event_cost;
        let model = event
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let entry = by_model
            .entry(model)
            .or_insert_with(|| json!({"calls": 0, "tokens": 0.0, "cost": 0.0}));
        entry["calls"] = json!(entry["calls"].as_u64().unwrap_or_default() + 1);
        entry["tokens"] = json!(entry["tokens"].as_f64().unwrap_or_default() + event_tokens);
        entry["cost"] = json!(entry["cost"].as_f64().unwrap_or_default() + event_cost);
    }
    json!({
        "path": path.display().to_string(),
        "events": calls,
        "tokens": tokens,
        "cost": cost,
        "byModel": Value::Object(by_model),
        "updatedAt": usage.get("updatedAt").cloned().unwrap_or(Value::Null),
    })
}

fn quota_json() -> Value {
    match fetch_subscription_quotas() {
        SubscriptionQuotas::Unavailable(reason) => json!({"available": false, "reason": reason}),
        SubscriptionQuotas::Accounts(accounts) => {
            let providers: Vec<Value> = accounts
                .iter()
                .map(|account| {
                    let entries: Vec<Value> = account
                        .entries
                        .iter()
                        .map(|entry| match entry {
                            QuotaEntry::Bucket(labeled) => {
                                let bucket = &labeled.bucket;
                                json!({
                                    "label": labeled.label,
                                    "state": bucket.state,
                                    "remaining": bucket.remaining,
                                    "limit": bucket.limit,
                                    "percentFree": match (bucket.remaining, bucket.limit) {
                                        (Some(remaining), Some(limit)) if limit > 0 => {
                                            Some(percent_free(remaining, limit))
                                        }
                                        _ => None,
                                    },
                                    "resetsAtMs": bucket.resets_at_ms,
                                })
                            }
                            QuotaEntry::Unavailable { label, error } => {
                                json!({"label": label, "error": error})
                            }
                        })
                        .collect();
                    json!({"provider": account.provider, "entries": entries})
                })
                .collect();
            json!({"available": true, "providers": providers})
        }
    }
}

fn sessions_json() -> Value {
    let root = crate::dirs_home().join(".jeden/sessions");
    let mut dirs: Vec<(String, std::time::SystemTime)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            dirs.push((entry.file_name().to_string_lossy().to_string(), modified));
        }
    }
    dirs.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    let recent: Vec<Value> = dirs.iter().take(5).map(|(name, _)| json!(name)).collect();
    json!({"count": dirs.len(), "recent": recent})
}

/// Full snapshot; shared by the text/JSON CLI output and the dashboard API.
pub(crate) fn stats_json(cwd: &Path) -> Value {
    json!({
        "version": crate::JEDEN_VERSION,
        "generatedAtMs": now_ms(),
        "cwd": cwd.display().to_string(),
        "usage": {
            "project": usage_file_totals(&cwd.join(".jeden/usage.json")),
            "user": usage_file_totals(&crate::dirs_home().join(".jeden/usage.json")),
        },
        "quota": quota_json(),
        "sessions": sessions_json(),
    })
}

fn stats_text(cwd: &Path) -> String {
    let stats = stats_json(cwd);
    let mut lines = vec![format!(
        "jeden {} · {}",
        stats["version"].as_str().unwrap_or(""),
        stats["cwd"].as_str().unwrap_or("")
    )];
    for scope in ["project", "user"] {
        let usage = &stats["usage"][scope];
        lines.push(format!(
            "usage ({scope}): {} events · {} tokens · cost {}",
            usage["events"].as_u64().unwrap_or_default(),
            usage["tokens"].as_f64().unwrap_or_default() as u64,
            usage["cost"].as_f64().unwrap_or_default(),
        ));
    }
    if stats["quota"]["available"].as_bool() == Some(true) {
        for provider in stats["quota"]["providers"].as_array().into_iter().flatten() {
            for entry in provider["entries"].as_array().into_iter().flatten() {
                let amount = match entry.get("error").and_then(Value::as_str) {
                    Some(error) => format!("unavailable: {error}"),
                    None => format!(
                        "state {} · remaining {} · limit {} · percent free {}",
                        entry["state"], entry["remaining"], entry["limit"], entry["percentFree"]
                    ),
                };
                lines.push(format!(
                    "quota {} · {}: {amount}",
                    provider["provider"].as_str().unwrap_or(""),
                    entry["label"].as_str().unwrap_or("")
                ));
            }
        }
    } else {
        lines.push(format!(
            "quota unavailable: {}",
            stats["quota"]["reason"].as_str().unwrap_or("unknown")
        ));
    }
    lines.push(format!(
        "sessions: {} (dashboard: jeden stats --serve)",
        stats["sessions"]["count"].as_u64().unwrap_or_default()
    ));
    lines.join("\n") + "\n"
}

/// CLI `jeden stats [--json] [--summary] [--serve [--port N]]`.
pub(crate) fn stats_command(args: &Args) -> Result<String, String> {
    let flag = |name: &str| args.positionals.iter().any(|part| part == name);
    if flag("--serve") {
        let named = args
            .positionals
            .iter()
            .position(|part| part == "--port")
            .map(|index| args.positionals.get(index + 1));
        let port = match named {
            None => None,
            Some(value) => Some(
                value
                    .and_then(|value| value.parse::<u16>().ok())
                    .ok_or_else(|| {
                        format!(
                    "jeden stats --serve --port takes a port number from 0 to 65535, not {}",
                    value.map_or("nothing", String::as_str)
                )
                    })?,
            ),
        };
        return serve(&args.cwd, port);
    }
    if args.json {
        let stats = stats_json(&args.cwd);
        return serde_json::to_string_pretty(&stats)
            .map(|text| text + "\n")
            .map_err(|error| error.to_string());
    }
    if flag("--summary") {
        let stats = stats_json(&args.cwd);
        let project = &stats["usage"]["project"];
        return Ok(format!(
            "{} events · {} tokens · cost {} · sessions {}",
            project["events"].as_u64().unwrap_or_default(),
            project["tokens"].as_f64().unwrap_or_default() as u64,
            project["cost"].as_f64().unwrap_or_default(),
            stats["sessions"]["count"].as_u64().unwrap_or_default(),
        ) + "\n");
    }
    Ok(stats_text(&args.cwd))
}
