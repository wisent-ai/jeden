use super::*;

pub(in crate::agent) struct SessionRecorder {
    id: String,
    dir: PathBuf,
    cwd: PathBuf,
    ready: bool,
}

impl SessionRecorder {
    pub(in crate::agent) fn new(cwd: &Path) -> Self {
        let id = stamp();
        Self { dir: session_root().join(&id), id, cwd: cwd.to_path_buf(), ready: false }
    }

    pub(in crate::agent) fn ensure(&mut self) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }
        fs::create_dir_all(self.dir.join("artifacts")).map_err(|e| e.to_string())?;
        let state_path = self.dir.join("state.json");
        if !state_path.exists() {
            let state = json!({ "id": self.id, "cwd": self.cwd, "startedAt": now_stamp() });
            fs::write(&state_path, serde_json::to_string_pretty(&state).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("transcript.jsonl"))
            .map_err(|e| e.to_string())?;
        self.ready = true;
        Ok(())
    }

    pub(in crate::agent) fn record(&mut self, event_type: &str, data: Value) -> Result<(), String> {
        self.ensure()?;
        let event = json!({ "ts": now_stamp(), "type": event_type, "data": data });
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("transcript.jsonl"))
            .map_err(|e| e.to_string())?;
        writeln!(file, "{}", event).map_err(|e| e.to_string())
    }

    /// Re-root this session's recorded workspace (backs /move): update the cwd
    /// and rewrite state.json so exports/recall reflect the new workspace.
    pub(in crate::agent) fn set_cwd(&mut self, cwd: &Path) -> Result<(), String> {
        self.cwd = cwd.to_path_buf();
        let state_path = self.dir.join("state.json");
        let mut state = fs::read_to_string(&state_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or_else(|| json!({ "id": self.id, "startedAt": now_stamp() }));
        if let Some(obj) = state.as_object_mut() {
            obj.insert("cwd".into(), json!(cwd));
        }
        if let Some(parent) = state_path.parent() { let _ = fs::create_dir_all(parent); }
        fs::write(&state_path, serde_json::to_string_pretty(&state).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())
    }

    pub(in crate::agent) fn artifact_dir(&self) -> PathBuf {
        self.dir.join("artifacts")
    }

    pub(in crate::agent) fn path(&self) -> PathBuf {
        self.dir.clone()
    }
}

fn stamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let suffix: String = rand::thread_rng().sample_iter(&Alphanumeric).take(6).map(char::from).collect();
    format!("{}-{}", secs, suffix)
}

pub(in crate::agent) fn now_stamp() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs().to_string()
}
