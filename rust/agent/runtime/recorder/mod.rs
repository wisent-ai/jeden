use super::*;

mod cwd;

pub(in crate::agent) struct SessionRecorder {
    id: String,
    dir: PathBuf,
    cwd: PathBuf,
    ready: bool,
    active_leaf: Option<String>,
    lineage: Option<(PathBuf, Option<String>)>,
    pending_tool_results: Option<(usize, Vec<Value>)>,
    /// The transcript, held open for appending while this process records
    /// the session: what a fleet supervisor (Oko) reads from the kernel to
    /// tell which session a running Jeden is, as it does for every harness.
    _held: Option<fs::File>,
}

impl SessionRecorder {
    pub(in crate::agent) fn new(cwd: &Path) -> Self {
        let id = stamp();
        Self {
            dir: session_root().join(&id),
            id,
            cwd: cwd.to_path_buf(),
            ready: false,
            active_leaf: None,
            lineage: None,
            pending_tool_results: None,
            _held: None,
        }
    }

    pub(in crate::agent) fn child(
        cwd: &Path,
        parent_session: PathBuf,
        parent_entry: Option<String>,
    ) -> Self {
        let mut recorder = Self::new(cwd);
        recorder.lineage = Some((parent_session, parent_entry));
        recorder
    }

    pub(in crate::agent) fn open(cwd: &Path, dir: &Path) -> Result<Self, String> {
        let state_path = dir.join("state.json");
        let state: Value = serde_json::from_slice(
            &fs::read(&state_path)
                .map_err(|error| format!("cannot read {}: {}", state_path.display(), error))?,
        )
        .map_err(|error| format!("invalid {}: {}", state_path.display(), error))?;
        let id = state
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                dir.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .ok_or_else(|| format!("{} has no session id", state_path.display()))?;
        let lineage = state
            .get("lineage")
            .and_then(Value::as_object)
            .map(|lineage| {
                let parent_session = lineage
                    .get("parentSession")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("{} has invalid parentSession", state_path.display()))?;
                let parent_entry = lineage
                    .get("parentEntry")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                Ok::<_, String>((parent_session, parent_entry))
            })
            .transpose()?;
        let active_leaf = crate::cli::sessions::session_active_leaf(dir)?;
        Ok(Self {
            id,
            dir: dir.to_path_buf(),
            cwd: cwd.to_path_buf(),
            ready: true,
            active_leaf,
            lineage,
            pending_tool_results: None,
            _held: Some(held(dir)?),
        })
    }

    pub(in crate::agent) fn ensure(&mut self) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }
        let artifacts = self.dir.join("artifacts");
        fs::create_dir_all(&artifacts)
            .map_err(|error| format!("cannot create {}: {error}", artifacts.display()))?;
        let state_path = self.dir.join("state.json");
        if !state_path.exists() {
            self.write_state(now_stamp())?;
        }
        self._held = Some(held(&self.dir)?);
        self.ready = true;
        if let Some((parent_session, parent_entry)) = self.lineage.clone() {
            self.record(
                "lineage",
                json!({ "parentSession": parent_session, "parentEntry": parent_entry }),
            )?;
        }
        Ok(())
    }

    fn write_state(&self, started_at: String) -> Result<(), String> {
        let state = json!({
            "version": crate::cli::sessions::SESSION_LEDGER_VERSION,
            "id": self.id,
            "cwd": self.cwd,
            "startedAt": started_at,
            "activeLeaf": self.active_leaf,
            "lineage": self.lineage.as_ref().map(|(session, entry)| json!({
                "parentSession": session,
                "parentEntry": entry,
            })),
        });
        let path = self.dir.join("state.json");
        let encoded = serde_json::to_string_pretty(&state)
            .map_err(|error| format!("cannot encode {}: {error}", path.display()))?;
        fs::write(&path, encoded + "\n")
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    pub(in crate::agent) fn record(&mut self, event_type: &str, data: Value) -> Result<(), String> {
        self.record_entry(event_type, data).map(|_| ())
    }

    pub(in crate::agent) fn record_entry(
        &mut self,
        event_type: &str,
        mut data: Value,
    ) -> Result<crate::cli::sessions::LedgerEntry, String> {
        self.ensure()?;
        if event_type == "action" {
            let count = data
                .pointer("/action/tools")
                .and_then(Value::as_array)
                .map(Vec::len);
            self.pending_tool_results = count
                .filter(|count| *count > 0)
                .map(|count| (count, Vec::with_capacity(count)));
        } else if event_type == "tool_result" {
            let result = data.get("result").cloned().unwrap_or(Value::Null);
            if self.pending_tool_results.is_some() {
                let completed = {
                    let (remaining, results) =
                        self.pending_tool_results.as_mut().expect("checked above");
                    results.push(result);
                    *remaining = remaining.saturating_sub(1);
                    if *remaining == 0 {
                        Some(crate::tool_runtime::format_tool_result(&json!(results)))
                    } else {
                        data["replayPending"] = json!(true);
                        None
                    }
                };
                if let Some(content) = completed {
                    data["replayMessage"] = json!(content);
                    self.pending_tool_results = None;
                }
            } else {
                data["replayMessage"] = json!(crate::tool_runtime::format_tool_result(&result));
            }
        }
        let entry =
            crate::cli::sessions::append_ledger_entry(&self.dir, now_stamp(), event_type, data)?;
        self.active_leaf = Some(entry.id.clone());
        Ok(entry)
    }

    pub(in crate::agent) fn record_context(
        &mut self,
        reason: &str,
        messages: &[Value],
    ) -> Result<(), String> {
        self.record(
            "context_snapshot",
            json!({ "reason": reason, "messages": messages }),
        )
    }

    pub(in crate::agent) fn record_checkpoint(
        &mut self,
        label: Option<String>,
        messages: &[Value],
    ) -> Result<String, String> {
        self.ensure()?;
        let entry =
            crate::cli::sessions::append_checkpoint_entry(&self.dir, now_stamp(), label, messages)?;
        self.active_leaf = Some(entry.id.clone());
        Ok(entry.id)
    }

    pub(in crate::agent) fn rewind(
        &mut self,
        checkpoint_id: &str,
    ) -> Result<(String, Vec<Value>), String> {
        self.ensure()?;
        let (entry, messages) =
            crate::cli::sessions::append_rewind_entry(&self.dir, now_stamp(), checkpoint_id)?;
        self.active_leaf = Some(entry.id.clone());
        self.pending_tool_results = None;
        Ok((entry.id, messages))
    }

    pub(in crate::agent) fn active_leaf(&self) -> Result<Option<String>, String> {
        if !self.ready && !self.dir.join("state.json").exists() {
            return Ok(self.active_leaf.clone());
        }
        crate::cli::sessions::session_active_leaf(&self.dir)
    }

    pub(in crate::agent) fn artifact_dir(&self) -> PathBuf {
        self.dir.join("artifacts")
    }

    pub(in crate::agent) fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(in crate::agent) fn path(&self) -> PathBuf {
        self.dir.clone()
    }
}

/// The session's transcript in `dir`, opened for appending (created when the
/// session is new) and kept so for as long as the recorder records it.
fn held(dir: &Path) -> Result<fs::File, String> {
    let transcript = dir.join("transcript.jsonl");
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .map_err(|error| format!("cannot open {} for append: {error}", transcript.display()))
}

fn stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let suffix: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(6)
        .map(char::from)
        .collect();
    format!("{}-{}", secs, suffix)
}

pub(crate) fn now_stamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}
