//! `jeden stats` — local usage/quota/session snapshot as text, `--json`, or a
//! self-contained local web dashboard (`--serve [--port N]`). Without
//! `--port` the operating system assigns a free port and the command prints
//! the address it bound. The dashboard binds the loopback address only, serves a single
//! HTML page plus a `/api/stats` JSON endpoint, with explicit snapshot refresh.

mod dashboard;

use dashboard::serve;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

use crate::control_plane::quota::{
    fetch_subscription_quotas, percent_free, QuotaEntry, SubscriptionQuotas,
};
use crate::Args;

use crate::slash::usage_ledger::{read_ledger, Agg};

fn event_totals(agg: &Agg) -> Value {
    json!({
        "calls": agg.calls,
        "tokens": agg.total,
        "cost": agg.cost.total,
    })
}

/// Totals of one usage ledger, read by the reader `/usage` and the status line
/// use. A missing ledger is no usage; one that cannot be read, is not JSON, or
/// holds an event of another shape is reported with its path and the
/// decoder's reason instead of being counted as zero.
fn usage_file_totals(path: &Path) -> Value {
    let shown = path.display().to_string();
    let ledger = match read_ledger(path) {
        Ok(ledger) => ledger,
        Err(error) => return json!({"path": shown, "error": error}),
    };
    let mut by_model: BTreeMap<&str, Agg> = BTreeMap::new();
    for event in &ledger.events {
        by_model.entry(event.model.as_str()).or_default().add(event);
    }
    let by_model: serde_json::Map<String, Value> = by_model
        .into_iter()
        .map(|(model, agg)| (model.to_string(), event_totals(&agg)))
        .collect();
    let totals = event_totals(&Agg::of(&ledger.events));
    json!({
        "path": shown,
        "events": totals["calls"],
        "tokens": totals["tokens"],
        "cost": totals["cost"],
        "byModel": Value::Object(by_model),
        "updatedAt": ledger.updated_at,
    })
}

/// A token count as the text output prints it: a whole number.
fn whole(value: &Value) -> String {
    match value.as_f64() {
        Some(number) => number.round().to_string(),
        None => value.to_string(),
    }
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
                                    // A share of a limit of nothing is no share.
                                    "percentFree": match (bucket.remaining, bucket.limit.and_then(std::num::NonZeroU64::new)) {
                                        (Some(remaining), Some(limit)) => {
                                            Some(percent_free(remaining, limit.get()))
                                        }
                                        _no_share => None,
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

/// Every persisted session. A missing root is no sessions; a root or an
/// entry that cannot be read is reported with its cause, not counted as none.
fn sessions_json() -> Value {
    let root = crate::dirs_home().join(".jeden/sessions");
    let mut dirs: Vec<(String, std::time::SystemTime)> = Vec::new();
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return json!({"count": dirs.len(), "recent": []})
        }
        Err(error) => return json!({"error": format!("cannot list {}: {error}", root.display())}),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                return json!({"error": format!("cannot list {}: {error}", root.display())})
            }
        };
        let path = entry.path();
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_not_session) => continue,
            Err(error) => return json!({"error": format!("cannot examine {}: {error}", path.display())}),
        }
        let modified = match entry.metadata().and_then(|meta| meta.modified()) {
            Ok(modified) => modified,
            Err(error) => {
                return json!({"error": format!("cannot read the time of {}: {error}", path.display())})
            }
        };
        dirs.push((entry.file_name().to_string_lossy().to_string(), modified));
    }
    dirs.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    let recent: Vec<Value> = dirs.iter().map(|(name, _)| json!(name)).collect();
    json!({"count": dirs.len(), "recent": recent})
}

/// Full snapshot; shared by the text/JSON CLI output and the dashboard API.
pub(crate) fn stats_json(cwd: &Path) -> Value {
    json!({
        "version": crate::JEDEN_VERSION,
        // An undatable snapshot says why in place, as every other part does.
        "generatedAtMs": match crate::clock::unix_millis() {
            Ok(now) => json!(now),
            Err(error) => json!({"error": format!("the snapshot cannot be dated: {error}")}),
        },
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
    let mut lines = vec![format!("jeden {} · {}", crate::JEDEN_VERSION, cwd.display())];
    for (scope, usage) in stats["usage"].as_object().into_iter().flatten() {
        if let Some(error) = usage.get("error").and_then(Value::as_str) {
            lines.push(format!("usage ({scope}) unreadable: {error}"));
            continue;
        }
        lines.push(format!(
            "usage ({scope}): {} events · {} tokens · cost {}",
            usage["events"],
            whole(&usage["tokens"]),
            usage["cost"],
        ));
    }
    if stats["quota"]["available"].as_bool() == Some(true) {
        for provider in stats["quota"]["providers"].as_array().into_iter().flatten() {
            for entry in provider["entries"].as_array().into_iter().flatten() {
                // An entry the quota report left unnamed is said as that,
                // rather than printed as a line with blank names.
                let (Some(name), Some(label)) = (provider["provider"].as_str(), entry["label"].as_str()) else {
                    lines.push(format!("quota entry unreadable: the quota report names no provider or no label for {entry}"));
                    continue;
                };
                let amount = match entry.get("error").and_then(Value::as_str) {
                    Some(error) => format!("unavailable: {error}"),
                    None => format!(
                        "state {} · remaining {} · limit {} · percent free {}",
                        entry["state"], entry["remaining"], entry["limit"], entry["percentFree"]
                    ),
                };
                lines.push(format!("quota {name} · {label}: {amount}"));
            }
        }
    } else {
        lines.push(match stats["quota"]["reason"].as_str() {
            Some(reason) => format!("quota unavailable: {reason}"),
            None => "quota unavailable; no reason was reported".to_string(),
        });
    }
    match stats["sessions"].get("error").and_then(Value::as_str) {
        Some(error) => lines.push(format!("sessions unreadable: {error}")),
        None => lines.push(format!(
            "sessions: {} (dashboard: jeden stats --serve)",
            stats["sessions"]["count"]
        )),
    }
    lines.join("\n") + "\n"
}

/// CLI `jeden stats [--json] [--summary] [--serve [--port N]]`.
pub(crate) fn stats_command(args: &Args) -> Result<String, String> {
    let flag = |name: &str| args.positionals.iter().any(|part| part == name);
    if flag("--serve") {
        // The word after `--port` is the port; `--port` as the last word
        // names none and is refused.
        let mut words = args.positionals.iter();
        let named = words.by_ref().find(|part| *part == "--port").map(|_flag| words.next());
        let port = match named {
            None => None,
            Some(None) => return Err("jeden stats --serve --port takes a port number, and none followed it".into()),
            Some(Some(value)) => Some(value.parse::<u16>().map_err(|error| {
                format!("jeden stats --serve --port takes a port number, not {value}: {error}")
            })?),
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
        if let Some(error) = project.get("error").and_then(Value::as_str) {
            return Err(format!("usage unreadable: {error}"));
        }
        if let Some(error) = stats["sessions"].get("error").and_then(Value::as_str) {
            return Err(format!("sessions unreadable: {error}"));
        }
        return Ok(format!(
            "{} events · {} tokens · cost {} · sessions {}",
            project["events"],
            whole(&project["tokens"]),
            project["cost"],
            stats["sessions"]["count"],
        ) + "\n");
    }
    Ok(stats_text(&args.cwd))
}
