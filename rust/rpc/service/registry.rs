use super::*;

impl<B: SessionBackend> SessionService<B> {
    pub fn new(
        backend: Arc<B>,
        tenants: TenantGuard,
        idempotency: IdempotencyStore,
        replay: ReplayStore,
        executor: Arc<BoundedExecutor>,
    ) -> Self {
        Self {
            backend,
            tenants,
            idempotency,
            replay,
            executor,
            sessions: Mutex::new(HashMap::new()),
            instance: instance_stamp(),
            next_session: AtomicU64::new(1),
            next_request: AtomicU64::new(1),
        }
    }

    pub fn create_session(&self, caller: &TenantPrincipal) -> Result<String, ServiceError> {
        self.tenants.register_session(&caller.tenant)?;
        let id = format!(
            "session-{}-{}",
            self.instance,
            self.next_session.fetch_add(1, Ordering::Relaxed)
        );
        let path = match self.backend.create(&caller.tenant, &id) {
            Ok(path) => path,
            Err(error) => {
                let _ = self.tenants.release_session(&caller.tenant);
                return Err(ServiceError::Runtime(error));
            }
        };
        self.sessions
            .lock()
            .map_err(|_| ServiceError::Runtime("session registry lock poisoned".into()))?
            .insert(
                id.clone(),
                SessionEntry {
                    tenant: caller.tenant.clone(),
                    path,
                },
            );
        Ok(id)
    }

    /// Every session the caller may see, newest first. With granted workspaces
    /// that is the host's own ledgers under `session_root()`; without one it is
    /// exactly what this tenant created through this daemon.
    pub fn list_sessions(
        &self,
        caller: &TenantPrincipal,
        limit: Option<usize>,
    ) -> Result<SessionListing, ServiceError> {
        let held = self.held_sessions()?;
        let mut sessions = Vec::new();
        let mut skipped = 0usize;
        if caller.workspaces().is_empty() {
            for (id, entry) in held.iter() {
                if entry.tenant != caller.tenant {
                    continue;
                }
                match self.summarize(id, &entry.path, true) {
                    Ok(summary) => sessions.push(summary),
                    Err(()) => skipped += 1,
                }
            }
        } else {
            let root = crate::session_root();
            let entries = std::fs::read_dir(&root).map_err(|error| {
                ServiceError::Runtime(format!("cannot read {}: {}", root.display(), error))
            })?;
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().into_owned();
                let cwd = match session_state(&dir) {
                    Ok(state) => state.cwd,
                    Err(_) => {
                        skipped += 1;
                        continue;
                    }
                };
                if !caller.grants_path(&cwd) {
                    continue;
                }
                match self.summarize(&id, &dir, held.contains_key(&id)) {
                    Ok(summary) => sessions.push(summary),
                    Err(()) => skipped += 1,
                }
            }
        }
        // Newest first; `startedAt` is a string of epoch seconds, so it is
        // compared numerically and anything unparseable sorts last.
        sessions.sort_by(|left, right| {
            let rank = |value: &str| value.parse::<u64>().ok();
            match (rank(&right.started_at), rank(&left.started_at)) {
                (Some(right), Some(left)) => right.cmp(&left),
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        if let Some(limit) = limit {
            sessions.truncate(limit);
        }
        Ok(SessionListing { sessions, skipped })
    }

    /// Resume one of the host's own sessions and register it under its own host
    /// session id, so `session/prompt`, `session/replay` and `session/cancel`
    /// address it exactly like a created session.
    pub fn open_session(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
    ) -> Result<SessionSummary, ServiceError> {
        let dir = self.resolve_granted_session(caller, session_id)?;
        if let Some(entry) = self.held_sessions()?.get(session_id) {
            if entry.tenant != caller.tenant {
                return Err(ServiceError::AccessDenied);
            }
            // Opening twice is idempotent: no second backend session, no second
            // quota unit.
            return self
                .summarize(session_id, &entry.path, true)
                .map_err(|()| unreadable_session(session_id));
        }
        self.tenants.register_session(&caller.tenant)?;
        let path = match self.backend.open(&caller.tenant, session_id, &dir) {
            Ok(path) => path,
            Err(error) => {
                let _ = self.tenants.release_session(&caller.tenant);
                return Err(ServiceError::Runtime(error));
            }
        };
        let summary = match self.summarize(session_id, &path, true) {
            Ok(summary) => summary,
            Err(()) => {
                let _ = self.tenants.release_session(&caller.tenant);
                return Err(unreadable_session(session_id));
            }
        };
        // Registered under the HOST session id, in the very map
        // `authorize_session` consults, so a following `session/prompt` reaches
        // the ledger the terminal wrote: the backend session behind this id was
        // resumed in place by `AgentSession::resume_in_place`
        // (rust/sdk/session.rs:135) and keeps appending to `path`.
        self.sessions
            .lock()
            .map_err(|_| ServiceError::Runtime("session registry lock poisoned".into()))?
            .insert(
                session_id.to_owned(),
                SessionEntry {
                    tenant: caller.tenant.clone(),
                    path,
                },
            );
        Ok(summary)
    }

    /// The replayed turns of a session the caller may open or already created.
    /// Read-only: it neither resumes nor registers the session. `limit` keeps
    /// the newest N turns and reports whether anything older was dropped.
    pub fn history(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        limit: Option<usize>,
    ) -> Result<(Vec<Value>, bool), ServiceError> {
        let dir = match self.held_sessions()?.get(session_id) {
            Some(entry) if entry.tenant == caller.tenant => entry.path.clone(),
            Some(_) => return Err(ServiceError::AccessDenied),
            None => self.resolve_granted_session(caller, session_id)?,
        };
        let mut turns = self
            .backend
            .turns(&dir)
            .map_err(|_| unreadable_session(session_id))?;
        let truncated = limit.is_some_and(|limit| turns.len() > limit);
        if let Some(limit) = limit {
            if truncated {
                let dropped = turns.len() - limit;
                turns.drain(..dropped);
            }
        }
        Ok((turns, truncated))
    }

    fn held_sessions(&self) -> Result<HashMap<String, SessionEntry>, ServiceError> {
        self.sessions
            .lock()
            .map_err(|_| ServiceError::Runtime("session registry lock poisoned".into()))
            .map(|sessions| sessions.clone())
    }

    /// `Err(())` means the ledger could not be read; callers either skip the row
    /// or turn it into a typed refusal.
    fn summarize(&self, session_id: &str, dir: &Path, open: bool) -> Result<SessionSummary, ()> {
        let state = session_state(dir).map_err(|_| ())?;
        let turns = self.backend.turns(dir).map_err(|_| ())?;
        Ok(SessionSummary {
            session_id: session_id.to_owned(),
            path: dir.to_path_buf(),
            cwd: state.cwd,
            started_at: state.started_at,
            turns: turns.len(),
            open,
        })
    }

    /// A session id is only a host session when it names one directory directly
    /// under `session_root()` whose recorded `cwd` sits in a granted workspace.
    fn resolve_granted_session(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
    ) -> Result<PathBuf, ServiceError> {
        if session_id.trim().is_empty() {
            return Err(ServiceError::InvalidRequest(
                "sessionId must not be empty".into(),
            ));
        }
        if caller.workspaces().is_empty() {
            return Err(ServiceError::AccessDenied);
        }
        let candidate = PathBuf::from(session_id);
        let mut components = candidate.components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(ServiceError::AccessDenied);
        }
        let dir = crate::session_root().join(session_id);
        if !dir.join("state.json").is_file() {
            return Err(ServiceError::AccessDenied);
        }
        let state = session_state(&dir).map_err(|_| ServiceError::AccessDenied)?;
        if !caller.grants_path(&state.cwd) {
            return Err(ServiceError::AccessDenied);
        }
        Ok(dir)
    }
}
