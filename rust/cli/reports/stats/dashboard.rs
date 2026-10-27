//! The local statistics dashboard: one self-contained page served from the
//! loopback address only, and the server that answers exactly two paths.
//!
//! Split out of `cli/reports/stats.rs`, which had grown past the module line
//! cap; the page asks for the same JSON the command prints, so the browser
//! and the terminal cannot show different numbers.

use super::stats_json;
use reqwest::StatusCode;
use std::io::Write;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use tiny_http::{Header, Request, Response, Server};

const DASHBOARD_HTML: &str = concat!(
    r#"<!doctype html>
<html><head><meta charset="utf-8"><title>jeden stats</title>
<style>
body{background:#0d1117;color:#e6edf3;font:14px/1.5 -apple-system,monospace;margin:2em auto;max-width:900px;padding:0 1em}
h1{font-size:1.3em}h2{font-size:1em;color:#8b949e;text-transform:uppercase;letter-spacing:.08em;margin-top:2em}
.card{background:#161b22;border:1px solid #30363d;border-radius:8px;padding:1em;margin:.6em 0}
.bar{height:10px;background:#21262d;border-radius:5px;overflow:hidden;margin:.3em 0}
.bar>div{height:100%;background:#3fb950}
.row{display:flex;justify-content:space-between;gap:1em}
.dim{color:#8b949e}.num{font-variant-numeric:tabular-nums}
.error{color:#ff7b72;white-space:pre-wrap;overflow-wrap:anywhere}
button{background:#21262d;color:inherit;border:1px solid #8b949e;border-radius:5px;padding:.4em .8em;cursor:pointer}button:disabled{opacity:.5;cursor:default}
</style></head><body>
<h1>jeden stats <span class="dim" id="ver"></span></h1>
<button id="refresh-stats" type="button">Refresh</button>
<p id="feedback" class="dim" role="status" aria-live="polite">No snapshot available.</p>
<p id="failure" class="error" role="alert"></p>
<h2>Quota</h2><div id="quota"></div>
<p class="dim">Null quota fields mean no value was reported, not an unlimited allowance. The quota state comes from the provider.</p>
<h2>Usage</h2><div id="usage"></div>
<h2>Sessions</h2><div id="sessions" class="card"></div>
<script>
"#,
    include_str!("dashboard.js"),
    r#"</script></body></html>"#
);

fn respond(request: Request, status: StatusCode, content_type: &str, body: String) {
    let url = request.url().to_owned();
    let Ok(header) = Header::from_bytes("content-type".as_bytes(), content_type.as_bytes()) else {
        eprintln!(
            "jeden stats dashboard: the answer to {url} is not sent: its content type {content_type:?} is not a valid header value"
        );
        return;
    };
    let response = Response::from_string(body)
        .with_status_code(status.as_u16())
        .with_header(header);
    if let Err(error) = request.respond(response) {
        eprintln!("jeden stats dashboard: the answer to {url} could not be sent: {error}");
    }
}

/// Serve the dashboard on `port` of the loopback address, or on a port the
/// operating system assigns (port zero) when none is named, and print the
/// address actually bound.
pub(super) fn serve(cwd: &Path, port: Option<u16>) -> Result<String, String> {
    let address = match port {
        Some(port) => format!("{}:{port}", Ipv4Addr::LOCALHOST),
        None => format!("{}:0", Ipv4Addr::LOCALHOST),
    };
    let server = Server::http(&address).map_err(|error| format!("cannot bind {address}: {error}"))?;
    println!(
        "jeden stats dashboard: http://{}  (Ctrl-C to stop)",
        server.server_addr()
    );
    std::io::stdout()
        .flush()
        .map_err(|error| format!("cannot print the dashboard's address: {error}"))?;
    let cwd: PathBuf = cwd.to_path_buf();
    for request in server.incoming_requests() {
        let cwd = cwd.clone();
        std::thread::spawn(move || {
            let path = match request.url().split_once('?') {
                Some((path, _query)) => path.to_owned(),
                None => request.url().to_owned(),
            };
            match path.as_str() {
                "/" => respond(
                    request,
                    StatusCode::OK,
                    "text/html; charset=utf-8",
                    DASHBOARD_HTML.to_owned(),
                ),
                "/api/stats" => {
                    let body = stats_json(&cwd).to_string();
                    respond(request, StatusCode::OK, "application/json", body);
                }
                _ => respond(
                    request,
                    StatusCode::NOT_FOUND,
                    "text/plain",
                    format!("{path} is not a dashboard page"),
                ),
            }
        });
    }
    Ok(String::new())
}
