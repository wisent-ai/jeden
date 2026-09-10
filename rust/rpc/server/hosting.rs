use super::*;
pub fn serve_headless_cli(positionals: &[String], data_root: &Path) -> Result<(), String> {
    if !(5..=6).contains(&positionals.len()) {
        return Err("Usage: jeden headless <addr> <server-cert.pem> <server-key.pem> <client-ca.pem> <identity-map.json> [revoked-serials.txt]".into());
    }
    let mappings: Vec<HeadlessIdentityMapping> = serde_json::from_slice(
        &fs::read(&positionals[4])
            .map_err(|error| format!("failed to read identity map: {error}"))?,
    )
    .map_err(|error| format!("invalid identity map: {error}"))?;
    if mappings.is_empty() {
        return Err("identity map must not be empty".into());
    }
    let directory = TenantDirectory::new();
    for mapping in mappings {
        let san = mapping.san.clone();
        directory
            .map_san(
                mapping.san,
                mapping.principal,
                mapping.tenant,
                mapping.workspaces,
            )
            .map_err(|error| match error {
                TenantError::InvalidWorkspace(message) => {
                    format!("invalid identity mapping for {san}: {message}")
                }
                _ => format!("invalid identity mapping for {san}"),
            })?;
    }
    let revoked_serials = positionals
        .get(5)
        .map(|path| read_revoked_serials(Path::new(path)))
        .transpose()?
        .unwrap_or_default();
    let tls = ReloadableTlsAcceptor::new(MtlsConfig {
        certificate_chain: PathBuf::from(&positionals[1]),
        private_key: PathBuf::from(&positionals[2]),
        client_ca_bundle: PathBuf::from(&positionals[3]),
        revoked_serials,
    })?;
    fs::create_dir_all(data_root)
        .map_err(|error| format!("failed to create headless data root: {error}"))?;
    let tenant_guard = TenantGuard::new(
        data_root.join("tenants"),
        TenantLimits {
            max_active_requests: 4,
            max_sessions: 32,
            max_stored_bytes: 1024 * 1024 * 1024,
        },
    );
    let backend = Arc::new(AgentSessionFacade::new(tenant_guard.clone()));
    let executor = Arc::new(BoundedExecutor::new(4, 64)?);
    let service = Arc::new(SessionService::new(
        backend,
        tenant_guard,
        IdempotencyStore::new(data_root.join("idempotency")),
        ReplayStore::new(data_root.join("replay"), 10_000),
        executor,
    ));
    let config = HeadlessConfig {
        reconnect_key: load_or_create_reconnect_key(&data_root.join("reconnect.key"))?,
        ..Default::default()
    };
    let daemon = Arc::new(HeadlessDaemon::new(tls, directory, service, config)?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&positionals[0])
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

fn load_or_create_reconnect_key(path: &Path) -> Result<Vec<u8>, String> {
    match fs::read(path) {
        Ok(key) if key.len() >= 32 => return Ok(key),
        Ok(_) => return Err("stored reconnect key is shorter than 32 bytes".into()),
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(format!("failed to read reconnect key: {error}"))
        }
        Err(_) => {}
    }
    let mut key = vec![0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
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
pub(super) fn quick_replies() -> Vec<Value> {
    let Ok(cwd) = std::env::current_dir() else {
        return Vec::new();
    };
    crate::capability::slash_descriptors(&cwd)
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
        .take(64)
        .collect()
}
