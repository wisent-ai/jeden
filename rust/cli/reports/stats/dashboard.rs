//! The local statistics dashboard: one self-contained page served from the
//! loopback address only, and the server that answers exactly two paths.
//!
//! Split out of `cli/reports/stats.rs`, which had grown past the module line
//! cap; the page asks for the same JSON the command prints, so the browser
//! and the terminal cannot show different numbers.

use super::stats_json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

const DASHBOARD_HTML: &str = concat!(r#"<!doctype html>
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
"#, include_str!("dashboard.js"), r#"</script></body></html>"#);

fn write_response(stream: &mut std::net::TcpStream, status: &str, content_type: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Serve the dashboard on `port`, or on a port the operating system assigns
/// when none is named, and print the address actually bound.
pub(super) fn serve(cwd: &Path, port: Option<u16>) -> Result<String, String> {
    let asked = port.unwrap_or_default();
    let listener = TcpListener::bind(("127.0.0.1", asked))
        .map_err(|error| format!("cannot bind 127.0.0.1:{asked}: {error}"))?;
    let bound = listener
        .local_addr()
        .map_err(|error| format!("cannot read the address 127.0.0.1:{asked} was bound to: {error}"))?;
    println!("jeden stats dashboard: http://{bound}  (Ctrl-C to stop)");
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
                "/" => write_response(
                    &mut stream,
                    "200 OK",
                    "text/html; charset=utf-8",
                    DASHBOARD_HTML,
                ),
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
