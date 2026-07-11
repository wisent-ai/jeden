use super::brama::{BramaClient, BramaError};
use super::weles::{InteractionBridge, LoginMethod, OperationEvent, WelesClient};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone)]
struct Response {
    status: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: String,
}

impl Response {
    fn json(body: Value) -> Self {
        Self { status: "200 OK", headers: vec![("Content-Type", "application/json")], body: body.to_string() }
    }

    fn with_header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }

    fn not_modified() -> Self {
        Self { status: "304 Not Modified", headers: Vec::new(), body: String::new() }
    }
}

#[derive(Clone, Debug)]
struct Request {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Value,
}

struct ScriptedServer {
    endpoint: String,
    requests: Arc<Mutex<Vec<Request>>>,
    thread: Option<JoinHandle<()>>,
}

impl ScriptedServer {
    fn start(responses: Vec<Response>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let thread = std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                if request.method.is_empty() { break; }
                captured.lock().push(request);
                let mut head = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    response.body.len()
                );
                for (name, value) in response.headers { head.push_str(&format!("{name}: {value}\r\n")); }
                head.push_str("\r\n");
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(response.body.as_bytes()).unwrap();
            }
        });
        Self { endpoint, requests, thread: Some(thread) }
    }

    fn finish(mut self) -> Vec<Request> {
        if !self.thread.as_ref().unwrap().is_finished() {
            if let Ok(stream) = TcpStream::connect(self.endpoint.trim_start_matches("http://")) {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        self.thread.take().unwrap().join().unwrap();
        self.requests.lock().clone()
    }
}

impl Drop for ScriptedServer {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            if !thread.is_finished() {
                if let Ok(stream) = TcpStream::connect(self.endpoint.trim_start_matches("http://")) {
                    let _ = stream.shutdown(Shutdown::Both);
                }
            }
            let _ = thread.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap_or(0);
        if count == 0 {
            return Request { method: String::new(), target: String::new(), headers: HashMap::new(), body: Value::Null };
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") { break position + 4; }
    };
    let header = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let mut lines = header.split("\r\n");
    let mut request_line = lines.next().unwrap().split_whitespace();
    let method = request_line.next().unwrap().to_string();
    let target = request_line.next().unwrap().to_string();
    let headers = lines.filter_map(|line| line.split_once(':')).map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string())).collect::<HashMap<_, _>>();
    let content_length = headers.get("content-length").and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);
    while bytes.len() - header_end < content_length {
        let count = stream.read(&mut buffer).unwrap();
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body = if content_length == 0 { Value::Null } else { serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap() };
    Request { method, target, headers, body }
}

#[derive(Default)]
struct Bridge {
    answer: String,
    elicited: Mutex<Vec<(String, Vec<String>, bool)>>,
    events: Mutex<Vec<OperationEvent>>,
}

impl InteractionBridge for Bridge {
    fn elicit(&self, prompt: &str, options: &[String], secret: bool) -> Result<String, String> {
        self.elicited.lock().push((prompt.to_string(), options.to_vec(), secret));
        Ok(self.answer.clone())
    }

    fn event(&self, event: &OperationEvent) { self.events.lock().push(event.clone()); }
}

fn account(id: &str, provider: &str, status: &str, refresh_required: bool) -> Value {
    json!({
        "id": id,
        "provider": provider,
        "displayName": format!("{provider} account"),
        "status": status,
        "expiresAt": "2030-01-02T03:04:05Z",
        "refreshRequired": refresh_required
    })
}

fn catalog(model: Value) -> Value { json!({"version": "v1", "models": [model]}) }

fn model(id: &str) -> Value {
    json!({
        "id": id,
        "available": true,
        "contextWindow": 131072,
        "maxOutputTokens": 16384,
        "inputModalities": ["text", "image"],
        "outputModalities": ["text"],
        "tools": true,
        "reasoning": true,
        "pricing": {"input": 1.25, "output": 5.5, "cacheRead": 0.1, "cacheWrite": 1.5},
        "fallback": ["backup/model"],
        "promotion": ["stable"]
    })
}

#[test]
fn control_plane_weles_discovers_login_methods_pastes_input_polls_cursor_and_returns_account_status() {
    let provider = json!({
        "id": "anthropic",
        "displayName": "Anthropic",
        "loginMethods": ["device_code", "paste", "api_key"],
        "available": true
    });
    let logged_in = account("acct-1", "anthropic", "active", false);
    let server = ScriptedServer::start(vec![
        Response::json(json!({"providers": [provider]})),
        Response::json(json!({"operationId": "login-1"})),
        Response::json(json!({
            "id": "login-1",
            "state": "pending",
            "cursor": "page 2",
            "events": [
                {"type": "status", "message": "waiting"},
                {"type": "deviceCode", "verification_uri": "https://local.invalid/device", "user_code": "ABCD", "expires_in_seconds": 60},
                {"type": "elicit", "field": "token", "prompt": "Paste token", "secret": true, "options": ["clipboard"]}
            ]
        })),
        Response::json(json!({})),
        Response::json(json!({"id": "login-1", "state": "completed", "events": [{"type": "completed", "account": logged_in}]})),
        Response::json(json!({"accounts": [logged_in]})),
    ]);
    let client = WelesClient::new(Some(server.endpoint.clone()), Some("weles-secret".into()), Duration::ZERO);
    let providers = client.providers().unwrap();
    assert_eq!(providers[0].login_methods, vec![LoginMethod::DeviceCode, LoginMethod::Paste, LoginMethod::ApiKey]);

    let bridge = Bridge { answer: "pasted-value".into(), ..Bridge::default() };
    let result = client.login_provider(&providers[0], "jeden:test-agent", &bridge, &|| false).unwrap().unwrap();
    assert_eq!((result.id.as_str(), result.status.as_str(), result.expires_at.as_deref()), ("acct-1", "active", Some("2030-01-02T03:04:05Z")));
    assert_eq!(bridge.elicited.lock().as_slice(), &[("Paste token".into(), vec!["clipboard".into()], true)]);
    assert!(matches!(bridge.events.lock().as_slice(), [OperationEvent::Status { .. }, OperationEvent::DeviceCode { .. }, OperationEvent::Elicit { .. }, OperationEvent::Completed { .. }]));

    let accounts = client.accounts(Some("anthropic/team")).unwrap();
    assert_eq!((accounts[0].provider.as_str(), accounts[0].status.as_str(), accounts[0].refresh_required), ("anthropic", "active", false));
    let requests = server.finish();
    assert_eq!(requests.iter().map(|request| (request.method.as_str(), request.target.as_str())).collect::<Vec<_>>(), vec![
        ("GET", "/v1/providers"),
        ("POST", "/v1/auth/login"),
        ("GET", "/v1/operations/login-1"),
        ("POST", "/v1/operations/login-1/input"),
        ("GET", "/v1/operations/login-1?cursor=page+2"),
        ("GET", "/v1/accounts?provider=anthropic%2Fteam"),
    ]);
    assert_eq!(requests[1].body, json!({"provider": "anthropic", "consumer": "jeden:test-agent"}));
    assert_eq!(requests[3].body, json!({"field": "token", "value": "pasted-value"}));
    assert!(requests.iter().all(|request| request.headers.get("authorization").map(String::as_str) == Some("Bearer weles-secret")));
}

#[test]
fn control_plane_weles_refreshes_only_due_accounts_then_logs_out_the_selected_account() {
    let refreshed_required = account("due-flag", "openai", "active", true);
    let refreshed_expiring = account("due-status", "google", "expiring", false);
    let logged_out = account("due-status", "google", "logged_out", false);
    let server = ScriptedServer::start(vec![
        Response::json(json!({"accounts": [account("healthy", "anthropic", "active", false), refreshed_required, refreshed_expiring]})),
        Response::json(json!({"operationId": "refresh-1"})),
        Response::json(json!({"id": "refresh-1", "state": "completed", "events": [{"type": "completed", "account": refreshed_required}]})),
        Response::json(json!({"operationId": "refresh-2"})),
        Response::json(json!({"id": "refresh-2", "state": "completed", "events": [{"type": "completed", "account": refreshed_expiring}]})),
        Response::json(json!({"operationId": "logout-1"})),
        Response::json(json!({"id": "logout-1", "state": "completed", "events": [{"type": "completed", "account": logged_out}]})),
    ]);
    let client = WelesClient::new(Some(server.endpoint.clone()), None, Duration::ZERO);
    let refreshed = client.refresh_due(&Bridge::default(), &|| false).unwrap();
    assert_eq!(refreshed.iter().map(|account| account.id.as_str()).collect::<Vec<_>>(), vec!["due-flag", "due-status"]);
    let result = client.logout("due-status", &Bridge::default(), &|| false).unwrap().unwrap();
    assert_eq!((result.id.as_str(), result.status.as_str()), ("due-status", "logged_out"));

    let requests = server.finish();
    let operation_bodies = requests.iter().filter(|request| request.method == "POST" && request.target.starts_with("/v1/auth/")).map(|request| (request.target.as_str(), request.body.clone())).collect::<Vec<_>>();
    assert_eq!(operation_bodies, vec![
        ("/v1/auth/refresh", json!({"accountId": "due-flag"})),
        ("/v1/auth/refresh", json!({"accountId": "due-status"})),
        ("/v1/auth/logout", json!({"accountId": "due-status"})),
    ]);
}

#[test]
fn control_plane_brama_preserves_catalog_metadata_and_rejects_unknown_and_unavailable_models() {
    let mut unavailable = model("retired/model");
    unavailable["available"] = json!(false);
    unavailable["unavailableReason"] = json!("entitlement missing");
    let server = ScriptedServer::start(vec![Response::json(json!({"version": "v1", "models": [model("vendor/model"), unavailable]}))]);
    let client = BramaClient::new(Some(server.endpoint.clone()), None, Duration::from_secs(60));
    let catalog = client.catalog(false).unwrap();
    let available = catalog.resolve("vendor/model").unwrap();
    assert_eq!((available.context_window, available.max_output_tokens, available.tools, available.reasoning), (131072, 16384, true, true));
    assert_eq!(available.input_modalities, ["text", "image"]);
    assert_eq!(available.output_modalities, ["text"]);
    assert_eq!((available.price.input, available.price.output, available.price.cache_read, available.price.cache_write), (1.25, 5.5, 0.1, 1.5));
    assert_eq!(available.fallback, ["backup/model"]);
    assert_eq!(available.promotion, ["stable"]);
    assert_eq!(catalog.resolve("missing/model"), Err(BramaError::UnknownModel("missing/model".into())));
    assert_eq!(catalog.resolve("retired/model"), Err(BramaError::UnavailableModel { model: "retired/model".into(), reason: "entitlement missing".into() }));
    assert!(catalog.price("retired/model").is_none());
    assert_eq!(server.finish().len(), 1);
}

#[test]
fn control_plane_brama_uses_ttl_then_revalidates_with_etag_and_reuses_304_catalog() {
    let server = ScriptedServer::start(vec![
        Response::json(catalog(model("vendor/cached"))).with_header("ETag", "\"catalog-v1\""),
        Response::not_modified(),
    ]);
    let client = BramaClient::new(Some(server.endpoint.clone()), Some("brama-secret".into()), Duration::from_secs(60));
    let first = client.catalog(false).unwrap();
    let ttl_hit = client.catalog(false).unwrap();
    assert_eq!(ttl_hit, first);
    let revalidated = client.catalog(true).unwrap();
    assert_eq!(revalidated, first);
    assert_eq!(client.catalog(false).unwrap(), first);

    let requests = server.finish();
    assert_eq!(requests.len(), 2, "fresh TTL and renewed 304 TTL must not issue extra requests");
    assert_eq!(requests[0].headers.get("authorization").map(String::as_str), Some("Bearer brama-secret"));
    assert_eq!(requests[1].headers.get("if-none-match").map(String::as_str), Some("\"catalog-v1\""));
}

#[test]
fn control_plane_weles_auth_completion_invalidates_the_brama_catalog_cache() {
    let brama = ScriptedServer::start(vec![
        Response::json(catalog(model("vendor/before"))),
        Response::json(catalog(model("vendor/after"))),
    ]);
    let brama_client = BramaClient::new(Some(brama.endpoint.clone()), None, Duration::from_secs(3600));
    assert!(brama_client.catalog(false).unwrap().resolve("vendor/before").is_ok());

    let weles = ScriptedServer::start(vec![
        Response::json(json!({"operationId": "logout-cache"})),
        Response::json(json!({"id": "logout-cache", "state": "completed", "events": [{"type": "completed"}]})),
    ]);
    let weles_client = WelesClient::new(Some(weles.endpoint.clone()), None, Duration::ZERO);
    assert_eq!(weles_client.logout("account-1", &Bridge::default(), &|| false).unwrap(), None);

    let refreshed = brama_client.catalog(false).unwrap();
    assert!(refreshed.resolve("vendor/after").is_ok());
    assert_eq!(brama.finish().len(), 2, "completed auth must invalidate a still-fresh catalog");
    assert_eq!(weles.finish().len(), 2);
}

static ENVIRONMENT: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Environment(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl Environment {
    fn brama(endpoint: &str) -> Self {
        let keys = ["BRAMA_URL", "BRAMA_TOKEN"];
        let previous = keys.into_iter().map(|key| (key, std::env::var_os(key))).collect();
        std::env::set_var("BRAMA_URL", endpoint);
        std::env::remove_var("BRAMA_TOKEN");
        Self(previous)
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value { Some(value) => std::env::set_var(key, value), None => std::env::remove_var(key) }
        }
    }
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("jeden-control-plane-{}-{}", std::process::id(), TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[test]
fn control_plane_unknown_model_is_rejected_by_interactive_resolution_and_before_persistence() {
    let _environment_lock = ENVIRONMENT.lock();
    let server = ScriptedServer::start(vec![Response::json(catalog(model("known/model")))]);
    let _environment = Environment::brama(&server.endpoint);
    let cwd = TempDirectory::new();

    let interactive = crate::cli::run::slash::resolve_model_route(&cwd.0, "unknown/model");
    assert!(interactive.is_err());
    let persisted = crate::cli::run::slash::handle_slash(&cwd.0, "/model unknown/model", None);
    assert!(persisted.is_err());
    assert!(!crate::config_path(&cwd.0).exists(), "a rejected route must not mutate persisted configuration");
    assert_eq!(server.finish().len(), 1, "the second resolver should use the same fresh catalog");
}
