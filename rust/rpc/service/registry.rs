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
            unrecorded: Mutex::new(HashMap::new()),
            instance: instance_stamp(),
            next_session: AtomicU64::new(crate::tally::FIRST),
            next_request: AtomicU64::new(crate::tally::FIRST),
        }
    }

    pub fn create_session(&self, caller: &TenantPrincipal) -> Result<String, ServiceError> {
        self.tenants.register_session(&caller.tenant)?;
        let id = format!(
            "session-{}-{}",
            self.instance,
            crate::tally::draw(&self.next_session)
        );
        let path = match self.backend.create(&caller.tenant, &id) {
            Ok(path) => path,
            Err(error) => {
                self.tenants.release_session(&caller.tenant);
                return Err(ServiceError::Runtime(error));
            }
        };
        // Whole-step inserts and clones: a panicking holder leaves a state.
        crate::panicked::held(&self.sessions).insert(
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
        let mut unreadable = Vec::new();
        if caller.workspaces().is_empty() {
            for (id, entry) in held.iter() {
                if entry.tenant != caller.tenant {
                    continue;
                }
                match self.summarize(id, &entry.path, true) {
                    Ok(summary) => sessions.push(summary),
                    Err(cause) => unreadable.push(UnreadableSession {
                        session_id: id.clone(),
                        cause,
                    }),
                }
            }
        } else {
            let root = crate::session_root();
            let entries = std::fs::read_dir(&root).map_err(|error| {
                ServiceError::Runtime(format!("cannot read {}: {}", root.display(), error))
            })?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    ServiceError::Runtime(format!("cannot read {}: {}", root.display(), error))
                })?;
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().into_owned();
                let cwd = match session_state(&dir) {
                    Ok(state) => state.cwd,
                    Err(cause) => {
                        unreadable.push(UnreadableSession {
                            session_id: id,
                            cause,
                        });
                        continue;
                    }
                };
                if !caller.grants_path(&cwd) {
                    continue;
                }
                match self.summarize(&id, &dir, held.contains_key(&id)) {
                    Ok(summary) => sessions.push(summary),
                    Err(cause) => unreadable.push(UnreadableSession {
                        session_id: id,
                        cause,
                    }),
                }
            }
        }
        // Newest first; `startedAt` is a string of epoch seconds, so it is
        // compared numerically and a value that is not a number sorts last.
        sessions.sort_by(|left, right| {
            match (
                right.started_at.parse::<u64>(),
                left.started_at.parse::<u64>(),
            ) {
                (Ok(right), Ok(left)) => right.cmp(&left),
                (Ok(_), Err(_unparsed)) => std::cmp::Ordering::Greater,
                (Err(_unparsed), Ok(_)) => std::cmp::Ordering::Less,
                (Err(_right), Err(_left)) => std::cmp::Ordering::Equal,
            }
        });
        if let Some(limit) = limit {
            sessions.truncate(limit);
        }
        Ok(SessionListing {
            sessions,
            unreadable,
        })
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
                .map_err(|cause| unreadable_session(session_id, &cause));
        }
        self.tenants.register_session(&caller.tenant)?;
        let path = match self.backend.open(&caller.tenant, session_id, &dir) {
            Ok(path) => path,
            Err(error) => {
                self.tenants.release_session(&caller.tenant);
                return Err(ServiceError::Runtime(error));
            }
        };
        let summary = match self.summarize(session_id, &path, true) {
            Ok(summary) => summary,
            Err(cause) => {
                self.tenants.release_session(&caller.tenant);
                return Err(unreadable_session(session_id, &cause));
            }
        };
        // Registered under the HOST session id, in the very map
        // `authorize_session` consults, so a following `session/prompt` reaches
        // the ledger the terminal wrote: the backend session behind this id was
        // resumed in place by `AgentSession::resume_in_place` and keeps
        // appending to `path`.
        crate::panicked::held(&self.sessions).insert(
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
            .map_err(|cause| unreadable_session(session_id, &cause))?;
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
        Ok(crate::panicked::held(&self.sessions).clone())
    }

    /// The listing row of one session, or why its ledger could not be read.
    fn summarize(
        &self,
        session_id: &str,
        dir: &Path,
        open: bool,
    ) -> Result<SessionSummary, String> {
        let state = session_state(dir)?;
        let turns = self.backend.turns(dir)?;
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
        // A caller not yet shown to own the session learns nothing more about
        // it than one it does not own: the same denial as an absent session.
        let state = session_state(&dir).map_err(|_unreadable| ServiceError::AccessDenied)?;
        if !caller.grants_path(&state.cwd) {
            return Err(ServiceError::AccessDenied);
        }
        Ok(dir)
    }
}
