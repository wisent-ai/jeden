//! `jeden stats` — local usage/quota/session snapshot as text, `--json`, or a
//! self-contained local web dashboard (`--serve [--port N]`, default 3847 like
//! omp's stats dashboard). The dashboard binds 127.0.0.1 only, serves a single
//! HTML page plus a `/api/stats` JSON endpoint, and refreshes itself.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use crate::control_plane::now_ms;
use crate::control_plane::quota::{
    fetch_subscription_quotas, percent_free, QuotaEntry, SubscriptionQuotas,
};
use crate::read_json;
use crate::Args;

const DEFAULT_PORT: u16 = 3847;

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
                ["inputTokens", "outputTokens", "cacheReadTokens", "cacheWriteTokens"]
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
    dirs.sort_by(|left, right| right.1.cmp(&left.1));
    let recent: Vec<Value> = dirs
        .iter()
        .take(5)
        .map(|(name, _)| json!(name))
        .collect();
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
        for provider in stats["quota"]["providers"].as_array().cloned().unwrap_or_default() {
            for entry in provider["entries"].as_array().cloned().unwrap_or_default() {
                let amount = match (
                    entry["remaining"].as_u64(),
                    entry["limit"].as_u64(),
                ) {
                    (Some(remaining), Some(limit)) if limit > 0 => {
                        format!("{remaining}/{limit} ({}% free)", entry["percentFree"])
                    }
                    _ => "unmetered".into(),
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

const DASHBOARD_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>jeden stats</title>
<style>
body{background:#0d1117;color:#e6edf3;font:14px/1.5 -apple-system,monospace;margin:2em auto;max-width:900px;padding:0 1em}
h1{font-size:1.3em}h2{font-size:1em;color:#8b949e;text-transform:uppercase;letter-spacing:.08em;margin-top:2em}
.card{background:#161b22;border:1px solid #30363d;border-radius:8px;padding:1em;margin:.6em 0}
.bar{height:10px;background:#21262d;border-radius:5px;overflow:hidden;margin:.3em 0}
.bar>div{height:100%;background:#3fb950}
.row{display:flex;justify-content:space-between;gap:1em}
.dim{color:#8b949e}.num{font-variant-numeric:tabular-nums}
</style></head><body>
<h1>jeden stats <span class="dim" id="ver"></span></h1>
<h2>Quota</h2><div id="quota"></div>
<h2>Usage</h2><div id="usage"></div>
<h2>Sessions</h2><div id="sessions" class="card"></div>
<script>
async function refresh(){
  const s = await (await fetch('/api/stats')).json();
  ver.textContent = 'v'+s.version;
  quota.innerHTML = s.quota.available
    ? s.quota.providers.map(p=>'<div class="card"><b>'+p.provider+'</b>'+p.entries.map(e=>{
        const pct = e.percentFree==null?null:e.percentFree;
        const bar = pct==null?'':'<div class="bar"><div style="width:'+pct+'%"></div></div>';
        const amt = e.remaining==null?'unmetered':e.remaining+(e.limit?' / '+e.limit:'')+(pct==null?'':' · '+pct+'% free');
        return '<div class="row"><span>'+e.label+'</span><span class="num dim">'+amt+'</span></div>'+bar;
      }).join('')+'</div>').join('')
    : '<div class="card dim">quota unavailable: '+(s.quota.reason||'')+'</div>';
  usage.innerHTML = ['project','user'].map(k=>{const u=s.usage[k];
    return '<div class="card"><b>'+k+'</b><div class="row"><span>'+u.events+' events</span><span class="num">'+Math.round(u.tokens)+' tokens</span><span class="num dim">cost '+u.cost.toFixed(4)+'</span></div></div>';
  }).join('');
  sessions.innerHTML = s.sessions.count+' sessions'+(s.sessions.recent.length?'<br><span class="dim">latest: '+s.sessions.recent.join(', ')+'</span>':'');
}
refresh(); setInterval(refresh, 5000);
</script></body></html>"#;

fn write_response(stream: &mut std::net::TcpStream, status: &str, content_type: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn serve(cwd: &Path, port: u16) -> Result<String, String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| format!("cannot bind 127.0.0.1:{port}: {error}"))?;
    println!("jeden stats dashboard: http://127.0.0.1:{port}  (Ctrl-C to stop)");
    let _ = std::io::stdout().flush();
    let cwd: PathBuf = cwd.to_path_buf();
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let cwd = cwd.clone();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or_default();
            let request = String::from_utf8_lossy(&buffer[..read]);
            let path = request
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .split('?')
                .next()
                .unwrap_or("/");
            match path {
                "/" => write_response(&mut stream, "200 OK", "text/html; charset=utf-8", DASHBOARD_HTML),
                "/api/stats" => {
                    let body = stats_json(&cwd).to_string();
                    write_response(&mut stream, "200 OK", "application/json", &body);
                }
                _ => write_response(&mut stream, "404 Not Found", "text/plain", "not found"),
            }
        });
    }
    Ok(String::new())
}

/// CLI `jeden stats [--json] [--summary] [--serve [--port N]]`.
pub(crate) fn stats_command(args: &Args) -> Result<String, String> {
    let flag = |name: &str| args.positionals.iter().any(|part| part == name);
    if flag("--serve") {
        let port = args
            .positionals
            .iter()
            .position(|part| part == "--port")
            .and_then(|index| args.positionals.get(index + 1))
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
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
