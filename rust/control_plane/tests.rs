use super::brama::{BramaClient, BramaError};
use super::contract::{
    BramaApiV1, ContractError, ModelRequest, RequestMeta, RouteRequest, WelesApiV1,
};
use super::transport::{ControlPlaneTransport, SecretRef, TransportRequest, TransportResponse};
use super::weles::{InteractionBridge, LoginMethod, OperationEvent, WelesClient, WelesError};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone)]
struct Response {
    status: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: String,
}

#[derive(Default)]
struct DeterministicTransport {
    responses: Mutex<VecDeque<Result<TransportResponse, String>>>,
    requests: Mutex<Vec<TransportRequest>>,
}

impl DeterministicTransport {
    fn new(responses: impl IntoIterator<Item = Result<TransportResponse, String>>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<TransportRequest> {
        self.requests.lock().clone()
    }
}

impl ControlPlaneTransport for DeterministicTransport {
    fn execute(&self, request: TransportRequest) -> Result<TransportResponse, String> {
        self.requests.lock().push(request);
        self.responses
            .lock()
            .pop_front()
            .expect("unexpected control-plane request")
    }
}

fn transport_response(status: u16, body: Value) -> Result<TransportResponse, String> {
    Ok(TransportResponse {
        status,
        headers: BTreeMap::new(),
        body: serde_json::to_vec(&body).unwrap(),
    })
}

fn transport_response_with_headers(
    status: u16,
    headers: &[(&str, &str)],
    body: Value,
) -> Result<TransportResponse, String> {
    Ok(TransportResponse {
        status,
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect(),
        body: serde_json::to_vec(&body).unwrap(),
    })
}

impl Response {
    fn json(body: Value) -> Self {
        Self {
            status: "200 OK",
            headers: vec![("Content-Type", "application/json")],
            body: body.to_string(),
        }
    }

    fn with_header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }

    fn not_modified() -> Self {
        Self {
            status: "304 Not Modified",
            headers: Vec::new(),
            body: String::new(),
        }
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
                if request.method.is_empty() {
                    break;
                }
                captured.lock().push(request);
                let mut head = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    response.body.len()
                );
                for (name, value) in response.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(response.body.as_bytes()).unwrap();
            }
        });
        Self {
            endpoint,
            requests,
            thread: Some(thread),
        }
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
                if let Ok(stream) = TcpStream::connect(self.endpoint.trim_start_matches("http://"))
                {
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
            return Request {
                method: String::new(),
                target: String::new(),
                headers: HashMap::new(),
                body: Value::Null,
            };
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let header = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let mut lines = header.split("\r\n");
    let mut request_line = lines.next().unwrap().split_whitespace();
    let method = request_line.next().unwrap().to_string();
    let target = request_line.next().unwrap().to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect::<HashMap<_, _>>();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() - header_end < content_length {
        let count = stream.read(&mut buffer).unwrap();
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body = if content_length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap()
    };
    Request {
        method,
        target,
        headers,
        body,
    }
}

#[derive(Default)]
struct Bridge {
    answer: String,
    elicited: Mutex<Vec<(String, Vec<String>, bool)>>,
    events: Mutex<Vec<OperationEvent>>,
}

impl InteractionBridge for Bridge {
    fn elicit(&self, prompt: &str, options: &[String], secret: bool) -> Result<String, String> {
        self.elicited
            .lock()
            .push((prompt.to_string(), options.to_vec(), secret));
        Ok(self.answer.clone())
    }

    fn event(&self, event: &OperationEvent) {
        self.events.lock().push(event.clone());
    }
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

fn catalog(model: Value) -> Value {
    json!({"version": "v1", "models": [model]})
}

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
fn control_plane_weles_discovers_login_methods_pastes_input_polls_cursor_and_returns_account_status(
) {
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
        Response::json(
            json!({"id": "login-1", "state": "completed", "events": [{"type": "completed", "account": logged_in}]}),
        ),
        Response::json(json!({"accounts": [logged_in]})),
    ]);
    let client = WelesClient::new(
        Some(server.endpoint.clone()),
        Some("weles-secret".into()),
        Duration::ZERO,
    );
    let providers = client.providers().unwrap();
    assert_eq!(
        providers[0].login_methods,
        vec![
            LoginMethod::DeviceCode,
            LoginMethod::Paste,
            LoginMethod::ApiKey
        ]
    );

    let bridge = Bridge {
        answer: "pasted-value".into(),
        ..Bridge::default()
    };
    let result = client
        .login_provider(&providers[0], "jeden:test-agent", &bridge, &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            result.id.as_str(),
            result.status.as_str(),
            result.expires_at.as_deref()
        ),
        ("acct-1", "active", Some("2030-01-02T03:04:05Z"))
    );
    assert_eq!(
        bridge.elicited.lock().as_slice(),
        &[("Paste token".into(), vec!["clipboard".into()], true)]
    );
    assert!(matches!(
        bridge.events.lock().as_slice(),
        [
            OperationEvent::Status { .. },
            OperationEvent::DeviceCode { .. },
            OperationEvent::Elicit { .. },
            OperationEvent::Completed { .. }
        ]
    ));

    let accounts = client.accounts(Some("anthropic/team")).unwrap();
    assert_eq!(
        (
            accounts[0].provider.as_str(),
            accounts[0].status.as_str(),
            accounts[0].refresh_required
        ),
        ("anthropic", "active", false)
    );
    let requests = server.finish();
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.method.as_str(), request.target.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("GET", "/v1/providers"),
            ("POST", "/v1/auth/login"),
            ("GET", "/v1/operations/login-1"),
            ("POST", "/v1/operations/login-1/input"),
            ("GET", "/v1/operations/login-1?cursor=page+2"),
            ("GET", "/v1/accounts?provider=anthropic%2Fteam"),
        ]
    );
    assert_eq!(
        requests[1].body,
        json!({"provider": "anthropic", "consumer": "jeden:test-agent"})
    );
    assert_eq!(
        requests[3].body,
        json!({"field": "token", "value": "pasted-value"})
    );
    assert!(requests.iter().all(|request| request
        .headers
        .get("authorization")
        .map(String::as_str)
        == Some("Bearer weles-secret")));
}

#[test]
fn control_plane_weles_refreshes_only_due_accounts_then_logs_out_the_selected_account() {
    let refreshed_required = account("due-flag", "openai", "active", true);
    let refreshed_expiring = account("due-status", "google", "expiring", false);
    let logged_out = account("due-status", "google", "logged_out", false);
    let server = ScriptedServer::start(vec![
        Response::json(
            json!({"accounts": [account("healthy", "anthropic", "active", false), refreshed_required, refreshed_expiring]}),
        ),
        Response::json(json!({"operationId": "refresh-1"})),
        Response::json(
            json!({"id": "refresh-1", "state": "completed", "events": [{"type": "completed", "account": refreshed_required}]}),
        ),
        Response::json(json!({"operationId": "refresh-2"})),
        Response::json(
            json!({"id": "refresh-2", "state": "completed", "events": [{"type": "completed", "account": refreshed_expiring}]}),
        ),
        Response::json(json!({"operationId": "logout-1"})),
        Response::json(
            json!({"id": "logout-1", "state": "completed", "events": [{"type": "completed", "account": logged_out}]}),
        ),
    ]);
    let client = WelesClient::new(Some(server.endpoint.clone()), None, Duration::ZERO);
    let refreshed = client.refresh_due(&Bridge::default(), &|| false).unwrap();
    assert_eq!(
        refreshed
            .iter()
            .map(|account| account.id.as_str())
            .collect::<Vec<_>>(),
        vec!["due-flag", "due-status"]
    );
    let result = client
        .logout("due-status", &Bridge::default(), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(
        (result.id.as_str(), result.status.as_str()),
        ("due-status", "logged_out")
    );

    let requests = server.finish();
    let operation_bodies = requests
        .iter()
        .filter(|request| request.method == "POST" && request.target.starts_with("/v1/auth/"))
        .map(|request| (request.target.as_str(), request.body.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        operation_bodies,
        vec![
            ("/v1/auth/refresh", json!({"accountId": "due-flag"})),
            ("/v1/auth/refresh", json!({"accountId": "due-status"})),
            ("/v1/auth/logout", json!({"accountId": "due-status"})),
        ]
    );
}

#[test]
fn control_plane_brama_preserves_catalog_metadata_and_rejects_unknown_and_unavailable_models() {
    let mut unavailable = model("retired/model");
    unavailable["available"] = json!(false);
    unavailable["unavailableReason"] = json!("entitlement missing");
    let server = ScriptedServer::start(vec![Response::json(
        json!({"version": "v1", "models": [model("vendor/model"), unavailable]}),
    )]);
    let client = BramaClient::new(Some(server.endpoint.clone()), None, Duration::from_secs(60));
    let catalog = client.catalog(false).unwrap();
    let available = catalog.resolve("vendor/model").unwrap();
    assert_eq!(
        (
            available.context_window,
            available.max_output_tokens,
            available.tools,
            available.reasoning
        ),
        (131072, 16384, true, true)
    );
    assert_eq!(available.input_modalities, ["text", "image"]);
    assert_eq!(available.output_modalities, ["text"]);
    assert_eq!(
        (
            available.price.input,
            available.price.output,
            available.price.cache_read,
            available.price.cache_write
        ),
        (1.25, 5.5, 0.1, 1.5)
    );
    assert_eq!(available.fallback, ["backup/model"]);
    assert_eq!(available.promotion, ["stable"]);
    assert_eq!(
        catalog.resolve("missing/model"),
        Err(BramaError::UnknownModel("missing/model".into()))
    );
    assert_eq!(
        catalog.resolve("retired/model"),
        Err(BramaError::UnavailableModel {
            model: "retired/model".into(),
            reason: "entitlement missing".into()
        })
    );
    assert!(catalog.price("retired/model").is_none());
    assert_eq!(server.finish().len(), 1);
}

#[test]
fn control_plane_brama_uses_ttl_then_revalidates_with_etag_and_reuses_304_catalog() {
    let server = ScriptedServer::start(vec![
        Response::json(catalog(model("vendor/cached"))).with_header("ETag", "\"catalog-v1\""),
        Response::not_modified(),
    ]);
    let client = BramaClient::new(
        Some(server.endpoint.clone()),
        Some("brama-secret".into()),
        Duration::from_secs(60),
    );
    let first = client.catalog(false).unwrap();
    let ttl_hit = client.catalog(false).unwrap();
    assert_eq!(ttl_hit, first);
    let revalidated = client.catalog(true).unwrap();
    assert_eq!(revalidated, first);
    assert_eq!(client.catalog(false).unwrap(), first);

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        2,
        "fresh TTL and renewed 304 TTL must not issue extra requests"
    );
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("Bearer brama-secret")
    );
    assert_eq!(
        requests[1].headers.get("if-none-match").map(String::as_str),
        Some("\"catalog-v1\"")
    );
}

#[test]
fn control_plane_weles_auth_completion_invalidates_the_brama_catalog_cache() {
    let brama = ScriptedServer::start(vec![
        Response::json(catalog(model("vendor/before"))),
        Response::json(catalog(model("vendor/after"))),
    ]);
    let brama_client = BramaClient::new(
        Some(brama.endpoint.clone()),
        None,
        Duration::from_secs(3600),
    );
    assert!(brama_client
        .catalog(false)
        .unwrap()
        .resolve("vendor/before")
        .is_ok());

    let weles = ScriptedServer::start(vec![
        Response::json(json!({"operationId": "logout-cache"})),
        Response::json(
            json!({"id": "logout-cache", "state": "completed", "events": [{"type": "completed"}]}),
        ),
    ]);
    let weles_client = WelesClient::new(Some(weles.endpoint.clone()), None, Duration::ZERO);
    assert_eq!(
        weles_client
            .logout("account-1", &Bridge::default(), &|| false)
            .unwrap(),
        None
    );

    let refreshed = brama_client.catalog(false).unwrap();
    assert!(refreshed.resolve("vendor/after").is_ok());
    assert_eq!(
        brama.finish().len(),
        2,
        "completed auth must invalidate a still-fresh catalog"
    );
    assert_eq!(weles.finish().len(), 2);
}

static ENVIRONMENT: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Environment(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl Environment {
    fn brama(endpoint: &str) -> Self {
        let keys = ["BRAMA_URL", "BRAMA_TOKEN"];
        let previous = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("BRAMA_URL", endpoint);
        std::env::remove_var("BRAMA_TOKEN");
        Self(previous)
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jeden-control-plane-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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
    assert!(
        !crate::config_path(&cwd.0).exists(),
        "a rejected route must not mutate persisted configuration"
    );
    assert_eq!(
        server.finish().len(),
        1,
        "the second resolver should use the same fresh catalog"
    );
}

#[test]
fn brama_v1_capabilities_resolve_and_stream_preserve_request_meta_and_normalize_usage() {
    let transport = DeterministicTransport::new([
        transport_response(200, json!({"capabilities": ["catalog", "stream", "usage"]})),
        transport_response(200, model("vendor/resolved")),
        transport_response(
            200,
            json!({
                "servedRoute": "vendor/resolved",
                "content": "certified output",
                "finishReason": "stop",
                "usage": {"inputTokens": 17, "outputTokens": 4, "costMicrounits": 23},
                "correlationId": "corr-stream"
            }),
        ),
    ]);
    let client = BramaClient::with_transport(
        Some("https://brama.invalid/root/".into()),
        Some("brama-token".into()),
        Duration::from_secs(60),
        transport.clone(),
    );
    let read_meta = RequestMeta::read("corr-capabilities");
    let resolve_meta = RequestMeta::mutation("corr-resolve", "idem-resolve");
    let stream_meta = RequestMeta::mutation("corr-stream", "idem-stream");

    assert_eq!(
        client.capabilities(&read_meta).unwrap(),
        ["catalog", "stream", "usage"]
    );
    let resolved = client
        .resolve(
            &RouteRequest {
                model: "vendor/requested".into(),
                required_modalities: vec!["text".into()],
                requires_tools: true,
            },
            &resolve_meta,
        )
        .unwrap();
    assert_eq!(resolved.id, "vendor/resolved");
    let streamed = client
        .stream(
            &ModelRequest {
                route: resolved.id,
                prompt: "hello".into(),
                max_output_tokens: 32,
            },
            &stream_meta,
            &|| false,
        )
        .unwrap();
    assert_eq!(
        (
            streamed.content.as_str(),
            streamed.served_route.as_str(),
            streamed.finish_reason.as_str()
        ),
        ("certified output", "vendor/resolved", "stop")
    );
    assert_eq!(
        (
            streamed.usage.input_tokens,
            streamed.usage.output_tokens,
            streamed.usage.cost_microunits
        ),
        (17, 4, 23)
    );

    let requests = transport.requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| (
                request.method.as_str(),
                request.url.as_str(),
                request.max_response_bytes
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                "GET",
                "https://brama.invalid/root/v1/capabilities",
                4 * 1024 * 1024
            ),
            (
                "POST",
                "https://brama.invalid/root/v1/resolve",
                4 * 1024 * 1024
            ),
            (
                "POST",
                "https://brama.invalid/root/v1/stream",
                4 * 1024 * 1024
            ),
        ]
    );
    for (request, correlation, idempotency) in [
        (&requests[0], "corr-capabilities", None),
        (&requests[1], "corr-resolve", Some("idem-resolve")),
        (&requests[2], "corr-stream", Some("idem-stream")),
    ] {
        assert_eq!(
            request.headers.get("x-correlation-id").map(String::as_str),
            Some(correlation)
        );
        assert_eq!(
            request.headers.get("idempotency-key").map(String::as_str),
            idempotency
        );
        assert_eq!(
            request
                .headers
                .get("x-jeden-schema-min")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            request
                .headers
                .get("x-jeden-schema-max")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer brama-token")
        );
    }
    assert_eq!(
        serde_json::from_slice::<Value>(requests[1].body.as_ref().unwrap()).unwrap(),
        json!({"model": "vendor/requested", "requiredModalities": ["text"], "requiresTools": true})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(requests[2].body.as_ref().unwrap()).unwrap(),
        json!({"route": "vendor/resolved", "prompt": "hello", "maxOutputTokens": 32})
    );
}

#[test]
fn weles_v1_explicit_lifecycle_preserves_cursor_payloads_and_mutation_idempotency() {
    let pending = |id: &str| json!({"id": id, "state": "pending"});
    let transport = DeterministicTransport::new([
        transport_response(200, json!({"operationId": "login-7"})),
        transport_response(
            200,
            json!({"id": "login-7", "state": "pending", "cursor": "next page", "events": [{"type": "status", "message": "waiting"}]}),
        ),
        transport_response(200, Value::Null),
        transport_response(409, json!({"secret": "must-not-leak"})),
        transport_response(200, pending("refresh-7")),
        transport_response(200, pending("logout-7")),
    ]);
    let client = WelesClient::with_transport(
        Some("https://weles.invalid/".into()),
        Some("weles-token".into()),
        Duration::ZERO,
        transport.clone(),
    );

    let login = WelesApiV1::begin_login(
        &client,
        "anthropic",
        "jeden:cert",
        &RequestMeta::mutation("corr-login", "idem-login"),
    )
    .unwrap();
    assert_eq!(
        (login.id.as_str(), login.state.as_str()),
        ("login-7", "pending")
    );
    let page = WelesApiV1::poll_operation(
        &client,
        "login-7",
        Some("previous page"),
        &RequestMeta::read("corr-poll"),
    )
    .unwrap();
    assert_eq!(
        (page.cursor.as_deref(), page.events.len()),
        (Some("next page"), 1)
    );
    WelesApiV1::submit_input(
        &client,
        "login-7",
        "token",
        "private-input",
        &RequestMeta::mutation("corr-input", "idem-input"),
    )
    .unwrap();
    WelesApiV1::cancel_operation(
        &client,
        "login-7",
        &RequestMeta::mutation("corr-cancel", "idem-cancel"),
    )
    .unwrap();
    assert_eq!(
        WelesApiV1::refresh(
            &client,
            "acct-7",
            &RequestMeta::mutation("corr-refresh", "idem-refresh")
        )
        .unwrap()
        .id,
        "refresh-7"
    );
    assert_eq!(
        WelesApiV1::logout(
            &client,
            "acct-7",
            &RequestMeta::mutation("corr-logout", "idem-logout")
        )
        .unwrap()
        .id,
        "logout-7"
    );

    let requests = transport.requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.method.as_str(), request.url.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("POST", "https://weles.invalid/v1/auth/login"),
            (
                "GET",
                "https://weles.invalid/v1/operations/login-7?cursor=previous+page"
            ),
            ("POST", "https://weles.invalid/v1/operations/login-7/input"),
            ("POST", "https://weles.invalid/v1/operations/login-7/cancel"),
            ("POST", "https://weles.invalid/v1/auth/refresh"),
            ("POST", "https://weles.invalid/v1/auth/logout"),
        ]
    );
    let expected = [
        ("corr-login", Some("idem-login")),
        ("corr-poll", None),
        ("corr-input", Some("idem-input")),
        ("corr-cancel", Some("idem-cancel")),
        ("corr-refresh", Some("idem-refresh")),
        ("corr-logout", Some("idem-logout")),
    ];
    for (request, (correlation, idempotency)) in requests.iter().zip(expected) {
        assert_eq!(
            request.headers.get("x-correlation-id").map(String::as_str),
            Some(correlation)
        );
        assert_eq!(
            request.headers.get("idempotency-key").map(String::as_str),
            idempotency
        );
        assert_eq!(
            request
                .headers
                .get("x-jeden-schema-min")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            request
                .headers
                .get("x-jeden-schema-max")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer weles-token")
        );
        assert_eq!(request.max_response_bytes, 2 * 1024 * 1024);
    }
    assert_eq!(
        serde_json::from_slice::<Value>(requests[0].body.as_ref().unwrap()).unwrap(),
        json!({"provider": "anthropic", "consumer": "jeden:cert"})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(requests[2].body.as_ref().unwrap()).unwrap(),
        json!({"field": "token", "value": "private-input"})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(requests[4].body.as_ref().unwrap()).unwrap(),
        json!({"accountId": "acct-7"})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(requests[5].body.as_ref().unwrap()).unwrap(),
        json!({"accountId": "acct-7"})
    );
}

#[test]
fn control_plane_status_taxonomy_suppresses_response_secrets_and_preserves_retry_after() {
    const SECRET: &str = "server-secret-must-never-appear";
    for status in [401_u16, 403, 409, 500, 503] {
        let brama_transport =
            DeterministicTransport::new([transport_response(status, json!({"error": SECRET}))]);
        let brama = BramaClient::with_transport(
            Some(format!("https://brama-{status}.invalid")),
            None,
            Duration::from_secs(1),
            brama_transport,
        );
        let error = brama
            .capabilities(&RequestMeta::read(format!("brama-{status}")))
            .unwrap_err();
        assert_eq!(
            error,
            BramaError::Http {
                status,
                message: "request failed; response body suppressed".into()
            }
        );
        assert!(!error.to_string().contains(SECRET));

        let weles_transport =
            DeterministicTransport::new([transport_response(status, json!({"error": SECRET}))]);
        let weles = WelesClient::with_transport(
            Some(format!("https://weles-{status}.invalid")),
            None,
            Duration::ZERO,
            weles_transport,
        );
        let error = WelesApiV1::begin_login(
            &weles,
            "provider",
            "consumer",
            &RequestMeta::mutation(format!("weles-{status}"), format!("idem-{status}")),
        )
        .unwrap_err();
        assert_eq!(
            error,
            WelesError::Http {
                status,
                message: "request failed; response body suppressed".into()
            }
        );
        assert!(!error.to_string().contains(SECRET));
    }

    let brama_transport = DeterministicTransport::new([transport_response_with_headers(
        429,
        &[("retry-after", "7")],
        json!({"error": SECRET}),
    )]);
    let brama = BramaClient::with_transport(
        Some("https://brama-rate.invalid".into()),
        None,
        Duration::from_secs(1),
        brama_transport,
    );
    assert_eq!(
        brama
            .capabilities(&RequestMeta::read("brama-rate"))
            .unwrap_err(),
        BramaError::RateLimited {
            retry_after_ms: Some(7000)
        }
    );

    let weles_transport = DeterministicTransport::new([transport_response_with_headers(
        429,
        &[("retry-after", "9")],
        json!({"error": SECRET}),
    )]);
    let weles = WelesClient::with_transport(
        Some("https://weles-rate.invalid".into()),
        None,
        Duration::ZERO,
        weles_transport,
    );
    assert_eq!(
        WelesApiV1::begin_login(
            &weles,
            "provider",
            "consumer",
            &RequestMeta::mutation("weles-rate", "idem-rate")
        )
        .unwrap_err(),
        WelesError::RateLimited {
            retry_after_ms: Some(9000)
        }
    );
}

#[test]
fn injected_transport_failures_malformed_payloads_and_stream_cancellation_are_observable() {
    let brama_transport = DeterministicTransport::new([
        Err("request timed out".into()),
        Err("response exceeds negotiated payload limit".into()),
        Ok(TransportResponse {
            status: 200,
            headers: BTreeMap::new(),
            body: b"not-json".to_vec(),
        }),
    ]);
    let brama = BramaClient::with_transport(
        Some("https://brama-failures.invalid".into()),
        None,
        Duration::from_secs(1),
        brama_transport.clone(),
    );
    assert_eq!(
        brama
            .capabilities(&RequestMeta::read("timeout"))
            .unwrap_err(),
        BramaError::Transport("request timed out".into())
    );
    assert_eq!(
        brama
            .capabilities(&RequestMeta::read("oversize"))
            .unwrap_err(),
        BramaError::Transport("response exceeds negotiated payload limit".into())
    );
    assert!(matches!(
        brama.capabilities(&RequestMeta::read("malformed")),
        Err(BramaError::InvalidResponse(_))
    ));
    let before_cancel = brama_transport.requests().len();
    assert_eq!(
        brama.stream(
            &ModelRequest {
                route: "vendor/model".into(),
                prompt: "stop".into(),
                max_output_tokens: 1
            },
            &RequestMeta::mutation("cancelled", "idem-cancelled"),
            &|| true
        ),
        Err(BramaError::Cancelled)
    );
    assert_eq!(
        brama_transport.requests().len(),
        before_cancel,
        "pre-cancelled streams must not reach the transport"
    );

    let completed_transport = DeterministicTransport::new([transport_response(
        200,
        json!({
            "servedRoute": "vendor/model",
            "content": "must be discarded",
            "finishReason": "stop",
            "usage": {"inputTokens": 1, "outputTokens": 1},
            "correlationId": "cancel-after-response"
        }),
    )]);
    let completed = BramaClient::with_transport(
        Some("https://brama-cancel-after.invalid".into()),
        None,
        Duration::from_secs(1),
        completed_transport.clone(),
    );
    let cancellation_checks = AtomicUsize::new(0);
    let cancelled_after_response = || cancellation_checks.fetch_add(1, Ordering::Relaxed) > 0;
    assert_eq!(
        completed.stream(
            &ModelRequest {
                route: "vendor/model".into(),
                prompt: "stop".into(),
                max_output_tokens: 1
            },
            &RequestMeta::mutation("cancel-after-response", "idem-cancel-after-response"),
            &cancelled_after_response
        ),
        Err(BramaError::Cancelled)
    );
    assert_eq!(
        completed_transport.requests().len(),
        1,
        "post-response cancellation must discard the terminal payload"
    );

    let weles_transport = DeterministicTransport::new([
        Err("request timed out".into()),
        Err("response exceeds negotiated payload limit".into()),
        Ok(TransportResponse {
            status: 200,
            headers: BTreeMap::new(),
            body: b"{".to_vec(),
        }),
        transport_response(200, json!({"id": "op-cancelled", "state": "cancelled"})),
    ]);
    let weles = WelesClient::with_transport(
        Some("https://weles-failures.invalid".into()),
        None,
        Duration::ZERO,
        weles_transport,
    );
    assert_eq!(
        WelesApiV1::begin_login(
            &weles,
            "p",
            "c",
            &RequestMeta::mutation("timeout", "idem-timeout")
        )
        .unwrap_err(),
        WelesError::Transport("request timed out".into())
    );
    assert_eq!(
        WelesApiV1::begin_login(
            &weles,
            "p",
            "c",
            &RequestMeta::mutation("oversize", "idem-oversize")
        )
        .unwrap_err(),
        WelesError::Transport("response exceeds negotiated payload limit".into())
    );
    assert!(matches!(
        WelesApiV1::begin_login(
            &weles,
            "p",
            "c",
            &RequestMeta::mutation("malformed", "idem-malformed")
        ),
        Err(WelesError::InvalidResponse(_))
    ));
    assert_eq!(
        WelesApiV1::poll_operation(
            &weles,
            "op-cancelled",
            None,
            &RequestMeta::read("poll-cancelled")
        ),
        Err(WelesError::Cancelled)
    );
}

#[test]
fn brama_catalog_returns_degraded_stale_data_after_transport_timeout() {
    BramaClient::invalidate_all();
    let transport = DeterministicTransport::new([
        transport_response_with_headers(
            200,
            &[("etag", "catalog-stale")],
            catalog(model("vendor/stale")),
        ),
        Err("request timed out".into()),
    ]);
    let client = BramaClient::with_transport(
        Some("https://brama-stale.invalid".into()),
        None,
        Duration::from_secs(3600),
        transport.clone(),
    );
    let fresh = client.catalog(false).unwrap();
    assert!(!fresh.degraded);
    let stale = client.catalog(true).unwrap();
    assert!(stale.degraded);
    assert_eq!(stale.resolve("vendor/stale").unwrap().id, "vendor/stale");
    let requests = transport.requests();
    assert_eq!(
        requests[1].headers.get("if-none-match").map(String::as_str),
        Some("catalog-stale")
    );
}

#[test]
fn environment_auth_is_late_bound_and_explicit_weles_completion_invalidates_brama_cache() {
    let _environment_lock = ENVIRONMENT.lock();
    const TOKEN_ENV: &str = "JEDEN_CONTROL_PLANE_TEST_TOKEN";
    let _environment = Environment(vec![(TOKEN_ENV, std::env::var_os(TOKEN_ENV))]);
    std::env::set_var(TOKEN_ENV, "first-token");

    let auth_transport = DeterministicTransport::new([
        transport_response(200, json!({"capabilities": ["first"]})),
        transport_response(200, json!({"capabilities": ["second"]})),
    ]);
    let auth_client = BramaClient::with_secret_ref(
        Some("https://brama-auth.invalid".into()),
        Some(SecretRef::environment(TOKEN_ENV)),
        Duration::from_secs(1),
        auth_transport.clone(),
    );
    assert_eq!(
        auth_client
            .capabilities(&RequestMeta::read("auth-first"))
            .unwrap(),
        ["first"]
    );
    std::env::set_var(TOKEN_ENV, "second-token");
    assert_eq!(
        auth_client
            .capabilities(&RequestMeta::read("auth-second"))
            .unwrap(),
        ["second"]
    );
    let auth_requests = auth_transport.requests();
    assert_eq!(
        auth_requests[0]
            .headers
            .get("authorization")
            .map(String::as_str),
        Some("Bearer first-token")
    );
    assert_eq!(
        auth_requests[1]
            .headers
            .get("authorization")
            .map(String::as_str),
        Some("Bearer second-token")
    );

    BramaClient::invalidate_all();
    let catalog_transport = DeterministicTransport::new([
        transport_response(200, catalog(model("vendor/before-explicit-poll"))),
        transport_response(200, catalog(model("vendor/after-explicit-poll"))),
    ]);
    let brama = BramaClient::with_transport(
        Some("https://brama-invalidation.invalid".into()),
        None,
        Duration::from_secs(3600),
        catalog_transport.clone(),
    );
    assert!(brama
        .catalog(false)
        .unwrap()
        .resolve("vendor/before-explicit-poll")
        .is_ok());
    let weles_transport = DeterministicTransport::new([transport_response(
        200,
        json!({"id": "done", "state": "completed"}),
    )]);
    let weles = WelesClient::with_transport(
        Some("https://weles-invalidation.invalid".into()),
        None,
        Duration::ZERO,
        weles_transport,
    );
    assert_eq!(
        WelesApiV1::poll_operation(&weles, "done", None, &RequestMeta::read("completion"))
            .unwrap()
            .state,
        "completed"
    );
    assert!(brama
        .catalog(false)
        .unwrap()
        .resolve("vendor/after-explicit-poll")
        .is_ok());
    assert_eq!(catalog_transport.requests().len(), 2);
}

#[test]
fn schema_and_mutation_metadata_are_rejected_at_contract_boundaries() {
    let transport = DeterministicTransport::new([]);
    let brama = BramaClient::with_transport(
        Some("https://brama-schema.invalid".into()),
        None,
        Duration::from_secs(1),
        transport.clone(),
    );
    let incompatible = RequestMeta {
        correlation_id: "schema-skew".into(),
        idempotency_key: None,
        schema_min: 2,
        schema_max: 3,
    };
    assert!(
        matches!(brama.capabilities(&incompatible), Err(BramaError::InvalidResponse(message)) if message.contains("schema negotiation failed"))
    );

    let weles = WelesClient::with_transport(
        Some("https://weles-schema.invalid".into()),
        None,
        Duration::ZERO,
        transport.clone(),
    );
    let missing_idempotency = RequestMeta::read("missing-idempotency");
    assert_eq!(
        WelesApiV1::begin_login(&weles, "provider", "consumer", &missing_idempotency),
        Err(WelesError::InvalidResponse(
            "mutation requires idempotency key".into()
        ))
    );
    assert!(transport.requests().is_empty());

    for (name, schema_min, schema_max) in [
        ("unsupported", "2", "3"),
        ("malformed", "not-a-version", "1"),
        ("inverted", "1", "0"),
    ] {
        let brama_transport = DeterministicTransport::new([transport_response_with_headers(
            200,
            &[
                ("x-jeden-schema-min", schema_min),
                ("x-jeden-schema-max", schema_max),
            ],
            json!({"capabilities": ["must-not-be-accepted"]}),
        )]);
        let brama = BramaClient::with_transport(
            Some(format!("https://brama-response-schema-{name}.invalid")),
            None,
            Duration::from_secs(1),
            brama_transport,
        );
        assert!(
            matches!(brama.capabilities(&RequestMeta::read(format!("brama-response-schema-{name}"))), Err(BramaError::InvalidResponse(message)) if message.contains("schema")),
            "Brama accepted {name} response schema range {schema_min}..={schema_max}"
        );

        let weles_transport = DeterministicTransport::new([transport_response_with_headers(
            200,
            &[
                ("x-jeden-schema-min", schema_min),
                ("x-jeden-schema-max", schema_max),
            ],
            json!({"operationId": "must-not-be-accepted"}),
        )]);
        let weles = WelesClient::with_transport(
            Some(format!("https://weles-response-schema-{name}.invalid")),
            None,
            Duration::ZERO,
            weles_transport,
        );
        assert!(
            matches!(WelesApiV1::begin_login(&weles, "provider", "consumer", &RequestMeta::mutation(format!("weles-response-schema-{name}"), format!("idem-response-schema-{name}"))), Err(WelesError::InvalidResponse(message)) if message.contains("schema")),
            "Weles accepted {name} response schema range {schema_min}..={schema_max}"
        );
    }
}

#[test]
#[ignore = "requires disposable Brama/Weles staging credentials and endpoints"]
fn control_plane_staging_e2e_writes_signed_report() {
    let report_path = std::env::var("JEDEN_CONTROL_PLANE_REPORT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("control-plane-e2e.json"));
    let evidence = super::staging::write_staging_report(&report_path)
        .unwrap_or_else(|error| panic!("control-plane staging blocked: {error}"));

    assert_eq!(evidence.status, "passed");
    assert_eq!(
        evidence.signature.len(),
        128,
        "Ed25519 signatures must be encoded as 64 bytes of hex"
    );
    assert!(
        evidence
            .signature
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()),
        "the report signature must be valid hex"
    );
    let digest = evidence
        .evidence_digest
        .strip_prefix("sha256:")
        .expect("the evidence digest must identify SHA-256");
    assert_eq!(digest.len(), 64);
    assert!(
        digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "the evidence digest must be valid hex"
    );
    assert!(
        !evidence.request_ids.is_empty() && evidence.request_ids.iter().all(|id| !id.is_empty()),
        "passed staging evidence must identify every certified request"
    );
    assert!(
        !evidence.operation_ids.is_empty()
            && evidence.operation_ids.iter().all(|id| !id.is_empty()),
        "passed staging evidence must identify every certified operation"
    );
    assert_eq!(super::staging::verify_staging_report(&evidence), Ok(()));
    let mut tampered = evidence.clone();
    tampered.served_route.push_str("-tampered");
    assert_eq!(
        super::staging::verify_staging_report(&tampered),
        Err("staging evidence digest mismatch".into())
    );
    let written: super::staging::StagingEvidence =
        serde_json::from_slice(&fs::read(&report_path).expect("staging report must be written"))
            .expect("staging report must be valid JSON");
    assert_eq!(written, evidence);
}

#[test]
fn weles_expiry_wins_when_cancellation_and_expiration_race() {
    let transport = DeterministicTransport::new([
        transport_response(
            200,
            json!({"id": "expired-pending", "state": "pending", "expiresAtMs": 1}),
        ),
        transport_response(
            200,
            json!({"id": "expired-cancelled", "state": "cancelled", "expiresAtMs": 1}),
        ),
    ]);
    let client = WelesClient::with_transport(
        Some("https://weles-expiry.invalid".into()),
        None,
        Duration::ZERO,
        transport,
    );

    assert_eq!(
        WelesApiV1::poll_operation(
            &client,
            "expired-pending",
            None,
            &RequestMeta::read("expiry-pending")
        ),
        Err(WelesError::ExpiredOperation)
    );
    assert_eq!(
        WelesApiV1::poll_operation(
            &client,
            "expired-cancelled",
            None,
            &RequestMeta::read("expiry-cancel-race")
        ),
        Err(WelesError::ExpiredOperation)
    );
}

#[test]
fn injected_stream_timeouts_distinguish_delayed_first_event_from_idle_stream() {
    let delayed_transport =
        DeterministicTransport::new([Err("stream timed out before first event".into())]);
    let delayed = BramaClient::with_transport(
        Some("https://brama-delayed.invalid".into()),
        None,
        Duration::from_secs(1),
        delayed_transport,
    );
    let request = ModelRequest {
        route: "vendor/model".into(),
        prompt: "hello".into(),
        max_output_tokens: 8,
    };
    assert_eq!(
        delayed.stream(
            &request,
            &RequestMeta::mutation("delayed-first-event", "idem-delayed-first-event"),
            &|| false
        ),
        Err(BramaError::Transport(
            "stream timed out before first event".into()
        ))
    );

    let idle_transport =
        DeterministicTransport::new([Err("stream idle timeout after first event".into())]);
    let idle = BramaClient::with_transport(
        Some("https://brama-idle.invalid".into()),
        None,
        Duration::from_secs(1),
        idle_transport,
    );
    assert_eq!(
        idle.stream(
            &request,
            &RequestMeta::mutation("idle-timeout", "idem-idle-timeout"),
            &|| false
        ),
        Err(BramaError::Transport(
            "stream idle timeout after first event".into()
        ))
    );
}

#[test]
fn staging_preflight_reports_every_missing_prerequisite_without_credentials() {
    let _environment_lock = ENVIRONMENT.lock();
    let keys = [
        "BRAMA_STAGING_URL",
        "WELES_STAGING_URL",
        "JEDEN_STAGING_OIDC_TOKEN",
        "JEDEN_STAGING_OIDC_AUDIENCE",
        "JEDEN_STAGING_OIDC_ROLE",
        "JEDEN_STAGING_TENANT",
        "JEDEN_STAGING_PROVIDER",
        "JEDEN_STAGING_MODEL",
        "JEDEN_STAGING_SCHEMA_MIN",
        "JEDEN_STAGING_SCHEMA_MAX",
        "JEDEN_STAGING_REPORT_SIGNING_KEY_HEX",
        "JEDEN_RELEASE_DIGEST",
    ];
    let credential_values = [
        "JEDEN_STAGING_OIDC_TOKEN",
        "JEDEN_STAGING_REPORT_SIGNING_KEY_HEX",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok())
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>();
    let _environment = Environment(
        keys.into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect(),
    );
    for key in keys {
        std::env::remove_var(key);
    }

    let error = match super::staging::staging_preflight_from_env() {
        Err(error) => error,
        Ok(_) => panic!("preflight unexpectedly accepted missing staging prerequisites"),
    };
    assert_eq!(error, ContractError::ExternalBlocked { prerequisites: vec![
        "BRAMA_STAGING_URL: Brama staging HTTPS endpoint".into(),
        "WELES_STAGING_URL: Weles staging HTTPS endpoint".into(),
        "JEDEN_STAGING_OIDC_TOKEN: short-lived workload OIDC credential for the configured audience/role".into(),
        "JEDEN_STAGING_OIDC_AUDIENCE: workload OIDC audience".into(),
        "JEDEN_STAGING_OIDC_ROLE: staging workload role".into(),
        "JEDEN_STAGING_TENANT: disposable staging tenant/account namespace".into(),
        "JEDEN_STAGING_PROVIDER: provider enabled for disposable lifecycle".into(),
        "JEDEN_STAGING_MODEL: harmless model route with quota".into(),
        "JEDEN_STAGING_SCHEMA_MIN: minimum supported staging schema version".into(),
        "JEDEN_STAGING_SCHEMA_MAX: maximum supported staging schema version".into(),
        "JEDEN_STAGING_REPORT_SIGNING_KEY_HEX: 32-byte short-lived Ed25519 report signing seed".into(),
        "JEDEN_RELEASE_DIGEST: immutable released canary digest under certification".into(),
    ]});
    let rendered = format!("{error:?}");
    assert!(
        credential_values
            .iter()
            .all(|value| !rendered.contains(value)),
        "staging prerequisite diagnostics must never disclose configured credential values"
    );
}

#[test]
fn transport_debug_output_suppresses_credentials_and_payload_bytes() {
    const AUTHORIZATION: &str = "Bearer authorization-secret-value";
    const REQUEST_SECRET: &str = "request-body-secret-value";
    const RESPONSE_SECRET: &str = "response-body-secret-value";
    let secret = SecretRef::inline(AUTHORIZATION);
    let request = TransportRequest {
        method: reqwest::Method::POST,
        url: "https://control-plane.invalid/v1/stream".into(),
        headers: BTreeMap::from([
            ("authorization".into(), AUTHORIZATION.into()),
            ("x-sensitive-value".into(), "sensitive-header-value".into()),
        ]),
        body: Some(serde_json::to_vec(&json!({"secret": REQUEST_SECRET})).unwrap()),
        max_response_bytes: 1024,
    };
    let response = TransportResponse {
        status: 401,
        headers: BTreeMap::from([(
            "x-sensitive-value".into(),
            "response-header-secret-value".into(),
        )]),
        body: serde_json::to_vec(&json!({"secret": RESPONSE_SECRET})).unwrap(),
    };

    let debug = format!("{secret:?}\n{request:?}\n{response:?}");
    for forbidden in [
        AUTHORIZATION,
        REQUEST_SECRET,
        RESPONSE_SECRET,
        "sensitive-header-value",
        "response-header-secret-value",
    ] {
        assert!(
            !debug.contains(forbidden),
            "control-plane Debug output disclosed `{forbidden}`"
        );
    }
    assert!(debug.contains("SecretRef([REDACTED])"));
}
