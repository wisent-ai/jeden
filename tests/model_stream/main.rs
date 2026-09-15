//! What a turn does while a route is thinking, driven through the real `jeden`
//! binary over real HTTP.
//!
//! Written after 2026-09-15, when seven assignments on one workstation ended
//! at `model stream first-event timeout`: the router gave the whole request
//! thirty seconds to produce its first streamed event, and `codex/gpt-6-astra`
//! needed longer than that to start answering a review prompt. The gateway
//! here is a socket this test owns, so "the model is slow to start" is a fact
//! the test states rather than a condition it hopes for.
//!
//! Run: `cargo test --test model_stream`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

/// Longer than the router's old first-event bound, short enough to keep the
/// case a test: the gateway holds its stream open and quiet for this long
/// before the first token.
const THINKING: Duration = Duration::from_secs(40);

/// What the model finally says.
const ANSWER: &str = "ready";

/// A gateway that accepts the request, answers `GET /v1/models` with a server
/// error so the run keeps its configured route, and holds the chat stream open
/// and quiet for `THINKING` before streaming one token and `[DONE]`.
fn slow_gateway() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local gateway");
    let port = listener.local_addr().expect("local address").port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            if serve(stream) {
                return;
            }
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// Answers one request; true once the chat stream has been served.
fn serve(mut stream: TcpStream) -> bool {
    let mut reader = BufReader::new(stream.try_clone().expect("clone the socket"));
    let mut request = String::new();
    reader.read_line(&mut request).expect("read the request line");
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).expect("read a header") == 0 {
            break;
        }
        let header = header.trim_end().to_ascii_lowercase();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    if request.starts_with("GET /v1/models") {
        let body = "{\"error\":\"the catalog is not part of this case\"}";
        let _ = write!(
            stream,
            "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.flush();
        return false;
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).expect("read the request body");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n"
    );
    let _ = stream.flush();
    thread::sleep(THINKING);
    let stream_body = format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{ANSWER}\"}}}}]}}\n\n\
         data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n\
         data: [DONE]\n\n"
    );
    let _ = stream.write_all(stream_body.as_bytes());
    let _ = stream.flush();
    true
}

struct Turn {
    root: PathBuf,
}

impl Turn {
    fn new(tag: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/model-stream")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for directory in ["home", "sessions", "workspace"] {
            std::fs::create_dir_all(root.join(directory)).expect("create the run's directories");
        }
        Self { root }
    }

    /// The operator's own choice of bound, written where a workspace keeps it.
    fn bound(&self, milliseconds: u64) {
        let directory = self.root.join("workspace/.jeden");
        std::fs::create_dir_all(&directory).expect("create the workspace config directory");
        let document = format!(
            "{{\"modelRouting\":{{\"retry\":{{\"firstEventTimeoutMs\":{milliseconds},\"maxAttempts\":1}}}}}}"
        );
        std::fs::write(directory.join("config.json"), document)
            .expect("write the workspace config");
    }

    fn run(&self, url: &str) -> (bool, String, Duration) {
        let started = Instant::now();
        let output = Command::new(env!("CARGO_BIN_EXE_jeden"))
            .args(["run", "say the word", "--model-only", "--model", "slow/route"])
            .env("HOME", self.root.join("home"))
            .env("JEDEN_SESSION_ROOT", self.root.join("sessions"))
            .env("JEDEN_TAMA_REGISTRY", "")
            .env("JEDEN_LANGUAGE", "en")
            .env("BRAMA_URL", url)
            .env("BRAMA_TOKEN", "local-gateway-token")
            .env("WISENT_APP_AGENT_ID", "jeden")
            .env("WISENT_APP_AGENT_AUTH_SECRET", "local-gateway-secret")
            .current_dir(self.root.join("workspace"))
            .output()
            .expect("run the jeden binary");
        let text = String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr);
        (output.status.success(), text, started.elapsed())
    }
}

/// The case the router used to lose: a gateway that accepts the request and
/// says nothing for longer than the old thirty-second bound, then answers.
#[test]
fn a_route_that_thinks_past_the_old_bound_still_answers() {
    let url = slow_gateway();
    let turn = Turn::new("thinking");
    let (succeeded, text, elapsed) = turn.run(&url);
    assert!(
        succeeded && text.contains(ANSWER),
        "the answer that arrived after {elapsed:?} of thinking was lost: {text}"
    );
    assert!(
        elapsed >= THINKING,
        "the gateway held its stream for {THINKING:?}; the turn returned in {elapsed:?}: {text}"
    );
    assert!(
        !text.contains("first-event timeout"),
        "a route that answered was reported as a timeout: {text}"
    );
}

/// An operator who names the bound keeps it: the same gateway, a bound below
/// its thinking time, and the refusal names the phase that expired.
#[test]
fn a_bound_the_operator_sets_is_the_bound() {
    let url = slow_gateway();
    let turn = Turn::new("configured");
    turn.bound(2_000);
    let (succeeded, text, elapsed) = turn.run(&url);
    assert!(
        !succeeded,
        "a bound the operator set must still expire: {text}"
    );
    assert!(
        text.contains("first-event timeout"),
        "the refusal must name the phase that expired: {text}"
    );
    assert!(
        elapsed < THINKING,
        "the turn waited {elapsed:?}, past the bound it was given: {text}"
    );
}
