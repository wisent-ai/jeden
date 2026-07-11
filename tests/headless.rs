use jeden::rpc::{
    BoundedExecutor, HeadlessConfig, HeadlessDaemon, IdempotencyStore, MtlsConfig,
    ReloadableTlsAcceptor, ReplayStore, SessionBackend, SessionService, TenantDirectory,
    TenantGuard, TenantId, TenantLimits, REQUIRED_ALPN,
};
use parking_lot::Mutex;
use rcgen::{
    date_time_ymd, BasicConstraints, Certificate, CertificateParams, IsCa, KeyPair, SerialNumber,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::io::{BufReader as StdBufReader, Cursor};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use x509_parser::parse_x509_certificate;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
const IO_DEADLINE: Duration = Duration::from_secs(3);

struct TempFixture(PathBuf);

impl TempFixture {
    fn new() -> Self {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("jeden-headless-{}-{sequence}", std::process::id()));
        fs::create_dir(&path).expect("create isolated headless fixture");
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, bytes).expect("write ephemeral TLS fixture");
        path
    }
}

impl Drop for TempFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone)]
struct Identity {
    cert_pem: String,
    key_pem: String,
}

struct Pki {
    temp: TempFixture,
    ca_pem: String,
    server_cert: PathBuf,
    server_key: PathBuf,
    good_a: Identity,
    good_b: Identity,
    wrong_ca: Identity,
    expired: Identity,
    revoked: Identity,
    revoked_serial: String,
}

impl Pki {
    fn generate() -> Self {
        let temp = TempFixture::new();
        let (ca, ca_key) = certificate_authority("Jeden test CA");
        let (wrong_ca, wrong_ca_key) = certificate_authority("Untrusted test CA");
        let ca_pem = ca.pem();
        let server = signed_identity(&ca, &ca_key, "localhost", 10, false);
        let good_a = signed_identity(&ca, &ca_key, "client-a.test", 11, false);
        let good_b = signed_identity(&ca, &ca_key, "client-b.test", 12, false);
        let untrusted = signed_identity(&wrong_ca, &wrong_ca_key, "client-wrong.test", 13, false);
        let expired = signed_identity(&ca, &ca_key, "client-expired.test", 14, true);
        let revoked = signed_identity(&ca, &ca_key, "client-revoked.test", 42, false);
        let revoked_der = parse_certificates(&revoked.cert_pem).remove(0);
        let (_, parsed) = parse_x509_certificate(revoked_der.as_ref())
            .expect("parse generated revoked certificate");
        let revoked_serial = parsed.raw_serial_as_string();
        let server_cert = temp.write("server.pem", &server.cert_pem);
        let server_key = temp.write("server.key", &server.key_pem);
        temp.write("ca.pem", &ca_pem);

        Self {
            temp,
            ca_pem,
            server_cert,
            server_key,
            good_a,
            good_b,
            wrong_ca: untrusted,
            expired,
            revoked,
            revoked_serial,
        }
    }

    fn mtls(&self, revoked: bool) -> MtlsConfig {
        MtlsConfig {
            certificate_chain: self.server_cert.clone(),
            private_key: self.server_key.clone(),
            client_ca_bundle: self.temp.path("ca.pem"),
            revoked_serials: if revoked {
                HashSet::from([self.revoked_serial.clone()])
            } else {
                HashSet::new()
            },
        }
    }
}

fn certificate_authority(name: &str) -> (Certificate, KeyPair) {
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("CA parameters");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, name);
    params.not_before = date_time_ymd(2024, 1, 1);
    params.not_after = date_time_ymd(2035, 1, 1);
    let key = KeyPair::generate().expect("generate CA key");
    let cert = params.self_signed(&key).expect("self-sign CA");
    (cert, key)
}

fn signed_identity(
    issuer: &Certificate,
    issuer_key: &KeyPair,
    san: &str,
    serial: u64,
    expired: bool,
) -> Identity {
    let mut params = CertificateParams::new(vec![san.to_owned()]).expect("identity parameters");
    params.serial_number = Some(SerialNumber::from(serial));
    params.not_before = if expired {
        date_time_ymd(2019, 1, 1)
    } else {
        date_time_ymd(2024, 1, 1)
    };
    params.not_after = if expired {
        date_time_ymd(2020, 1, 1)
    } else {
        date_time_ymd(2035, 1, 1)
    };
    let key = KeyPair::generate().expect("generate identity key");
    let cert = params
        .signed_by(&key, issuer, issuer_key)
        .expect("sign identity");
    Identity {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    }
}

fn parse_certificates(pem: &str) -> Vec<CertificateDer<'static>> {
    rustls_pemfile::certs(&mut StdBufReader::new(Cursor::new(pem.as_bytes())))
        .collect::<Result<Vec<_>, _>>()
        .expect("parse generated certificate PEM")
}

fn parse_private_key(pem: &str) -> PrivateKeyDer<'static> {
    rustls_pemfile::private_key(&mut StdBufReader::new(Cursor::new(pem.as_bytes())))
        .expect("parse generated private key PEM")
        .expect("generated PEM contains a private key")
}

fn client_config(
    server_ca_pem: &str,
    identity: Option<&Identity>,
    alpns: &[&str],
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    for certificate in parse_certificates(server_ca_pem) {
        roots.add(certificate).expect("trust generated server CA");
    }
    let builder =
        ClientConfig::builder_with_protocol_versions(versions).with_root_certificates(roots);
    let mut config = match identity {
        Some(identity) => builder
            .with_client_auth_cert(
                parse_certificates(&identity.cert_pem),
                parse_private_key(&identity.key_pem),
            )
            .expect("configure generated client identity"),
        None => builder.with_no_client_auth(),
    };
    config.alpn_protocols = alpns
        .iter()
        .map(|value| value.as_bytes().to_vec())
        .collect();
    Arc::new(config)
}

#[derive(Default)]
struct TestBackend {
    aborted: Mutex<Vec<(String, String, String)>>,
}

impl SessionBackend for TestBackend {
    fn create(&self, tenant: &TenantId, session_id: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(format!("{}/{session_id}", tenant.as_str())))
    }

    fn prompt(
        &self,
        tenant: &TenantId,
        _session_id: &str,
        request_id: &str,
        prompt: &str,
        emit: Arc<dyn Fn(String, Value, bool) + Send + Sync>,
    ) -> Result<Value, String> {
        emit("status".into(), json!({"message": "accepted"}), false);
        emit(
            "result".into(),
            json!({"text": format!("{}:{prompt}", tenant.as_str())}),
            true,
        );
        Ok(json!({"answer": format!("{}:{prompt}", tenant.as_str()), "requestId": request_id}))
    }

    fn abort(&self, tenant: &TenantId, session_id: &str, request_id: &str) -> Result<bool, String> {
        self.aborted.lock().push((
            tenant.as_str().to_owned(),
            session_id.to_owned(),
            request_id.to_owned(),
        ));
        Ok(true)
    }
}

struct RunningDaemon {
    addr: std::net::SocketAddr,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<Result<(), String>>,
    backend: Arc<TestBackend>,
}

async fn start_daemon(pki: &Pki, revoked: bool, max_connections: usize) -> RunningDaemon {
    let backend = Arc::new(TestBackend::default());
    let guard = TenantGuard::new(
        pki.temp.path("tenants"),
        TenantLimits {
            max_active_requests: 4,
            max_sessions: 8,
            max_stored_bytes: 1024 * 1024,
        },
    );
    let service = Arc::new(SessionService::new(
        backend.clone(),
        guard,
        IdempotencyStore::new(pki.temp.path("idempotency")),
        ReplayStore::new(pki.temp.path("replay"), 100),
        Arc::new(BoundedExecutor::new(2, 8).expect("create bounded executor")),
    ));
    let directory = TenantDirectory::new();
    directory
        .map_san("client-a.test", "principal-a", "tenant-a")
        .expect("map first client");
    directory
        .map_san("client-b.test", "principal-b", "tenant-b")
        .expect("map second client");
    directory
        .map_san("client-revoked.test", "principal-r", "tenant-r")
        .expect("map revoked client");
    let tls =
        ReloadableTlsAcceptor::new(pki.mtls(revoked)).expect("load ephemeral TLS configuration");
    let daemon = Arc::new(
        HeadlessDaemon::new(
            tls,
            directory,
            service,
            HeadlessConfig {
                max_frame_bytes: 64 * 1024,
                read_timeout: IO_DEADLINE,
                write_timeout: IO_DEADLINE,
                drain_timeout: IO_DEADLINE,
                max_connections,
                reconnect_key: vec![7; 32],
                reconnect_ttl: Duration::from_secs(60),
            },
        )
        .expect("construct headless daemon"),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral ingress");
    let addr = listener.local_addr().expect("read ephemeral address");
    let (shutdown, receiver) = watch::channel(false);
    let task = tokio::spawn(daemon.serve(listener, receiver));
    RunningDaemon {
        addr,
        shutdown,
        task,
        backend,
    }
}

async fn stop_daemon(daemon: RunningDaemon) {
    daemon.shutdown.send(true).expect("signal daemon shutdown");
    tokio::time::timeout(IO_DEADLINE, daemon.task)
        .await
        .expect("daemon drains before deadline")
        .expect("daemon task joins")
        .expect("daemon exits cleanly");
}

async fn connect_tls(
    addr: std::net::SocketAddr,
    config: Arc<ClientConfig>,
) -> Result<TlsStream<TcpStream>, String> {
    let tcp = TcpStream::connect(addr)
        .await
        .map_err(|error| error.to_string())?;
    let name = ServerName::try_from("localhost").expect("valid test server name");
    tokio::time::timeout(IO_DEADLINE, TlsConnector::from(config).connect(name, tcp))
        .await
        .map_err(|_| "TLS connect timed out".to_owned())?
        .map_err(|error| error.to_string())
}

struct RpcClient {
    stream: BufReader<TlsStream<TcpStream>>,
    next_id: u64,
}

impl RpcClient {
    async fn connect(addr: std::net::SocketAddr, config: Arc<ClientConfig>) -> Self {
        Self {
            stream: BufReader::new(
                connect_tls(addr, config)
                    .await
                    .expect("complete TLS handshake"),
            ),
            next_id: 1,
        }
    }

    async fn call_with_key(&mut self, method: &str, params: Value, key: &str) -> Value {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let frame = json!({
            "id": id,
            "method": method,
            "params": params,
            "meta": {
                "protocolVersion": REQUIRED_ALPN,
                "idempotencyKey": key,
                "deadlineUnixMillis": null,
                "traceId": format!("trace-{}", self.next_id)
            }
        });
        let mut encoded = serde_json::to_vec(&frame).expect("encode RPC frame");
        encoded.push(b'\n');
        tokio::time::timeout(IO_DEADLINE, self.stream.get_mut().write_all(&encoded))
            .await
            .expect("RPC write deadline")
            .expect("write RPC frame");
        let mut line = String::new();
        tokio::time::timeout(IO_DEADLINE, self.stream.read_line(&mut line))
            .await
            .expect("RPC read deadline")
            .expect("read RPC frame");
        assert!(
            !line.is_empty(),
            "server closed before returning RPC response"
        );
        let response: Value = serde_json::from_str(&line).expect("decode RPC response");
        assert_eq!(response["id"], id);
        response
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let key = format!("key-{}", self.next_id);
        self.call_with_key(method, params, &key).await
    }
}

async fn assert_ingress_rejected(
    addr: std::net::SocketAddr,
    config: Arc<ClientConfig>,
    case: &str,
) {
    match connect_tls(addr, config).await {
        Err(_) => {}
        Ok(mut stream) => {
            let probe = b"{\"id\":\"probe\"}\n";
            let _ = stream.write_all(probe).await;
            let mut byte = [0_u8; 1];
            let read = tokio::time::timeout(IO_DEADLINE, stream.read(&mut byte)).await;
            assert!(
                matches!(read, Ok(Ok(0)) | Ok(Err(_))),
                "{case}: rejected TLS peer remained able to exchange application data"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tls13_mtls_alpn_session_flow_and_cross_tenant_denial_are_enforced_on_the_wire() {
    let pki = Pki::generate();
    let daemon = start_daemon(&pki, false, 4).await;
    let config_a = client_config(
        &pki.ca_pem,
        Some(&pki.good_a),
        &[REQUIRED_ALPN],
        &[&rustls::version::TLS13],
    );
    let config_b = client_config(
        &pki.ca_pem,
        Some(&pki.good_b),
        &[REQUIRED_ALPN],
        &[&rustls::version::TLS13],
    );
    let mut client_a = RpcClient::connect(daemon.addr, config_a).await;
    let mut client_b = RpcClient::connect(daemon.addr, config_b).await;

    let created = client_a.call("session/create", json!({})).await;
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("create returns session id")
        .to_owned();
    assert!(created["result"]["reconnectToken"].as_str().is_some());

    let started = client_a
        .call_with_key(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": "hello"}),
            "prompt-once",
        )
        .await;
    assert_eq!(started["result"]["state"], "started");
    let request_id = started["result"]["requestId"]
        .as_str()
        .expect("prompt returns request id")
        .to_owned();

    let events = tokio::time::timeout(IO_DEADLINE, async {
        loop {
            let replay = client_a
                .call(
                    "session/replay",
                    json!({"sessionId": session_id, "requestId": request_id, "limit": 10}),
                )
                .await;
            let events = replay["result"]["events"]
                .as_array()
                .unwrap_or_else(|| panic!("replay did not return events: {replay}"));
            if events.iter().any(|event| event["terminal"] == true) {
                break events.clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal replay event was not observed before the I/O deadline");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["kind"], "status");
    assert_eq!(events[0]["payload"], json!({"message": "accepted"}));
    assert_eq!(events[1]["kind"], "result");
    assert_eq!(events[1]["payload"], json!({"text": "tenant-a:hello"}));
    assert_eq!(events[1]["terminal"], true);

    let completed = client_a
        .call_with_key(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": "hello"}),
            "prompt-once",
        )
        .await;
    assert_eq!(completed["result"]["state"], "completed");
    assert_eq!(completed["result"]["requestId"], request_id);
    assert_eq!(completed["result"]["result"]["answer"], "tenant-a:hello");

    let cancelled = client_a
        .call(
            "session/cancel",
            json!({"sessionId": session_id, "requestId": request_id}),
        )
        .await;
    assert_eq!(cancelled["result"]["cancelled"], true);
    assert_eq!(
        daemon.backend.aborted.lock().as_slice(),
        &[("tenant-a".into(), session_id.clone(), request_id.clone())]
    );

    let denied = client_b
        .call(
            "session/replay",
            json!({"sessionId": session_id, "requestId": request_id}),
        )
        .await;
    assert_eq!(denied["error"]["code"], "access_denied");
    assert_eq!(denied["error"]["message"], "access denied");

    drop(client_a);
    drop(client_b);
    stop_daemon(daemon).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_ingress_rejects_plaintext_and_invalid_tls_client_evidence() {
    let pki = Pki::generate();
    let daemon = start_daemon(&pki, true, 8).await;

    let mut plaintext = TcpStream::connect(daemon.addr)
        .await
        .expect("connect plaintext probe");
    plaintext
        .write_all(b"{\"method\":\"readiness\"}\n")
        .await
        .expect("write plaintext probe");
    let mut byte = [0_u8; 1];
    let read = tokio::time::timeout(IO_DEADLINE, plaintext.read(&mut byte)).await;
    assert!(
        matches!(read, Ok(Ok(0)) | Ok(Err(_))),
        "plaintext ingress was not closed"
    );

    let cases = [
        (
            "missing client certificate",
            client_config(
                &pki.ca_pem,
                None,
                &[REQUIRED_ALPN],
                &[&rustls::version::TLS13],
            ),
        ),
        (
            "wrong ALPN",
            client_config(
                &pki.ca_pem,
                Some(&pki.good_a),
                &["http/1.1"],
                &[&rustls::version::TLS13],
            ),
        ),
        (
            "client signed by wrong CA",
            client_config(
                &pki.ca_pem,
                Some(&pki.wrong_ca),
                &[REQUIRED_ALPN],
                &[&rustls::version::TLS13],
            ),
        ),
        (
            "expired client certificate",
            client_config(
                &pki.ca_pem,
                Some(&pki.expired),
                &[REQUIRED_ALPN],
                &[&rustls::version::TLS13],
            ),
        ),
        (
            "revoked client certificate",
            client_config(
                &pki.ca_pem,
                Some(&pki.revoked),
                &[REQUIRED_ALPN],
                &[&rustls::version::TLS13],
            ),
        ),
        (
            "TLS 1.2",
            client_config(
                &pki.ca_pem,
                Some(&pki.good_a),
                &[REQUIRED_ALPN],
                &[&rustls::version::TLS12],
            ),
        ),
    ];
    for (name, config) in cases {
        assert_ingress_rejected(daemon.addr, config, name).await;
    }

    stop_daemon(daemon).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_admission_returns_backpressure_without_attempting_tls() {
    let pki = Pki::generate();
    let daemon = start_daemon(&pki, false, 1).await;
    let config = client_config(
        &pki.ca_pem,
        Some(&pki.good_a),
        &[REQUIRED_ALPN],
        &[&rustls::version::TLS13],
    );
    let first = RpcClient::connect(daemon.addr, config).await;

    let mut overloaded = TcpStream::connect(daemon.addr)
        .await
        .expect("connect over capacity");
    let mut line = String::new();
    tokio::time::timeout(
        IO_DEADLINE,
        BufReader::new(&mut overloaded).read_line(&mut line),
    )
    .await
    .expect("overload response deadline")
    .expect("read overload response");
    let response: Value = serde_json::from_str(&line).expect("overload response is framed JSON");
    assert_eq!(response["error"]["code"], "backpressure");
    assert_eq!(response["error"]["retryable"], true);
    assert_eq!(response["error"]["details"]["retryAfterMillis"], 100);

    drop(first);
    stop_daemon(daemon).await;
}
