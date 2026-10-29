use super::*;
use std::num::{NonZeroU64, NonZeroUsize};

const USAGE: &str = "Usage: jeden headless <addr> <server-cert.pem> <server-key.pem> <client-ca.pem> <identity-map.json> <limits.json> [revoked-serials.txt]";

/// A reconnect key as long as the HMAC-SHA256 output that signs reconnect
/// tokens, the length below which RFC 2104 discourages a key:
/// https://www.rfc-editor.org/rfc/rfc2104#section-3 "less than L bytes is strongly discouraged"
const RECONNECT_KEY_BYTES: usize = 32;

/// Every bound one headless host keeps, as its operator declares them in the
/// limits file; none is compiled in, a missing or zero one is refused by the
/// parse naming the field, and an unknown one is refused too.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostLimits {
    /// Requests one tenant may run at once.
    max_active_requests: NonZeroUsize,
    /// Sessions one tenant may hold.
    max_sessions: NonZeroUsize,
    /// Bytes one tenant may store under the data root.
    max_stored_bytes: NonZeroU64,
    /// Threads session work runs on.
    workers: NonZeroUsize,
    /// Jobs that may wait for a thread before a submit is answered as backpressure.
    queue: NonZeroUsize,
    /// Events each session's replay keeps.
    replay_events: NonZeroUsize,
    /// The largest frame a connection may send.
    max_frame_bytes: NonZeroUsize,
    /// Connections served at once; one more is answered as overloaded.
    max_connections: NonZeroUsize,
    /// How long a reconnect token stays valid.
    reconnect_ttl_seconds: NonZeroU64,
}

fn host_limits(path: &str) -> Result<HostLimits, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("the host limits {path} cannot be read: {error}"))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("the host limits {path} are not a limits declaration: {error}"))
}

pub fn serve_headless_cli(positionals: &[String], data_root: &Path) -> Result<(), String> {
    let (address, certificate, key, client_ca, identity_map, limits, revoked) = match positionals {
        [address, certificate, key, client_ca, identity_map, limits] => (
            address,
            certificate,
            key,
            client_ca,
            identity_map,
            limits,
            None,
        ),
        [address, certificate, key, client_ca, identity_map, limits, revoked] => (
            address,
            certificate,
            key,
            client_ca,
            identity_map,
            limits,
            Some(revoked),
        ),
        _ => return Err(USAGE.into()),
    };
    let limits = host_limits(limits)?;
    let mappings: Vec<HeadlessIdentityMapping> = serde_json::from_slice(
        &fs::read(identity_map).map_err(|error| format!("failed to read identity map: {error}"))?,
    )
    .map_err(|error| format!("invalid identity map: {error}"))?;
    if mappings.is_empty() {
        return Err("identity map must not be empty".into());
    }
    let directory = TenantDirectory::new();
    for mapping in mappings {
        let label = match (&mapping.san, &mapping.wisent_organization) {
            (Some(san), _) => san.clone(),
            (None, Some(organization)) => format!("wisent organization {organization}"),
            (None, None) => format!("tenant {}", mapping.tenant),
        };
        let mapped = match (mapping.san, mapping.principal, mapping.wisent_organization) {
            (Some(san), Some(principal), None) => {
                directory.map_san(san, principal, mapping.tenant, mapping.workspaces)
            }
            (None, None, Some(organization)) => directory.map_wisent_organization(
                organization,
                mapping.tenant,
                mapping.workspaces,
            ),
            _ => {
                return Err(format!(
                    "invalid identity mapping for {label}: name either san and principal, or wisentOrganization"
                ))
            }
        };
        mapped.map_err(|error| match error {
            TenantError::InvalidWorkspace(message) => {
                format!("invalid identity mapping for {label}: {message}")
            }
            other => format!("invalid identity mapping for {label}: {other:?}"),
        })?;
    }
    let revoked_serials = match revoked {
        Some(path) => read_revoked_serials(Path::new(path))?,
        None => HashSet::new(),
    };
    let admits_members = directory.admits_wisent_members();
    let tls = ReloadableTlsAcceptor::new(MtlsConfig {
        certificate_chain: PathBuf::from(certificate),
        private_key: PathBuf::from(key),
        client_ca_bundle: PathBuf::from(client_ca),
        revoked_serials,
        admit_without_certificate: admits_members,
    })?;
    fs::create_dir_all(data_root)
        .map_err(|error| format!("failed to create headless data root: {error}"))?;
    let tenant_guard = TenantGuard::new(
        data_root.join("tenants"),
        TenantLimits {
            max_active_requests: limits.max_active_requests.get(),
            max_sessions: limits.max_sessions.get(),
            max_stored_bytes: limits.max_stored_bytes.get(),
        },
    );
    let backend = Arc::new(AgentSessionFacade::new(tenant_guard.clone()));
    let executor = Arc::new(BoundedExecutor::new(limits.workers, limits.queue)?);
    let service = Arc::new(SessionService::new(
        backend,
        tenant_guard,
        IdempotencyStore::new(data_root.join("idempotency")),
        ReplayStore::new(data_root.join("replay"), limits.replay_events),
        executor,
    ));
    let config = HeadlessConfig::new(
        limits.max_frame_bytes,
        limits.max_connections,
        load_or_create_reconnect_key(&data_root.join("reconnect.key"))?,
        std::time::Duration::from_secs(limits.reconnect_ttl_seconds.get()),
    );
    let daemon = HeadlessDaemon::new(tls, directory, service, config)?;
    let daemon = Arc::new(if admits_members {
        daemon.with_wisent_identity(WisentIdentityAuthority::from_environment()?)
    } else {
        daemon
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            format!("the secure headless listener's async runtime cannot be started: {error}")
        })?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|error| format!("failed to bind secure headless listener: {error}"))?;
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        daemon.serve(listener, shutdown_rx).await
    })
}

fn read_revoked_serials(path: &Path) -> Result<HashSet<String>, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read revocation list: {error}"))?;
    Ok(contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

/// The kept reconnect key, or a new one from the operating system's random
/// source written owner-only. A kept key too short to sign with is refused
/// by the daemon that signs with it.
fn load_or_create_reconnect_key(path: &Path) -> Result<Vec<u8>, String> {
    use rand::Rng;
    match fs::read(path) {
        Ok(key) => return Ok(key),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "failed to read reconnect key {}: {error}",
                path.display()
            ))
        }
    }
    let key: Vec<u8> = std::iter::repeat_with(|| rand::rngs::OsRng.gen::<u8>())
        .take(RECONNECT_KEY_BYTES)
        .collect();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = fs::OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(u32::from(libc::S_IRUSR | libc::S_IWUSR));
        let mut file = options
            .open(path)
            .map_err(|error| format!("failed to create reconnect key: {error}"))?;
        file.write_all(&key)
            .map_err(|error| format!("failed to persist reconnect key: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("failed to sync reconnect key: {error}"))?;
    }
    #[cfg(not(unix))]
    fs::write(path, &key).map_err(|error| format!("failed to persist reconnect key: {error}"))?;
    Ok(key)
}

/// The slash commands of the working directory a client may offer as quick
/// replies; a working directory that cannot be read is refused, not answered
/// as one with none.
pub(super) fn quick_replies() -> Result<Vec<Value>, String> {
    let cwd = std::env::current_dir().map_err(|error| {
        format!("the working directory whose slash commands are offered cannot be read: {error}")
    })?;
    Ok(crate::capability::slash_descriptors(&cwd)
        .into_iter()
        .filter_map(|descriptor| {
            if !descriptor.ui.visible || !descriptor.ui.executable {
                return None;
            }
            let crate::capability::FunctionTarget::FileSlash { command, .. } = &descriptor.target
            else {
                return None;
            };
            let prompt = descriptor.ui.action.clone()?;
            Some(json!({
                "id": descriptor.id,
                "label": command,
                "prompt": prompt,
                "source": descriptor.source,
            }))
        })
        .collect())
}
