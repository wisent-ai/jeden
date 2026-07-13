use super::*;

pub(in crate::agent) struct SessionRecorder {
    id: String,
    dir: PathBuf,
    cwd: PathBuf,
    ready: bool,
    active_leaf: Option<String>,
    lineage: Option<(PathBuf, Option<String>)>,
    pending_tool_results: Option<(usize, Vec<Value>)>,
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
        })
    }

    pub(in crate::agent) fn ensure(&mut self) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }
        fs::create_dir_all(self.dir.join("artifacts")).map_err(|e| e.to_string())?;
        let state_path = self.dir.join("state.json");
        if !state_path.exists() {
            self.write_state(now_stamp())?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("transcript.jsonl"))
            .map_err(|e| e.to_string())?;
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
        fs::write(
            self.dir.join("state.json"),
            serde_json::to_string_pretty(&state).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| e.to_string())
    }

    pub(in crate::agent) fn record(&mut self, event_type: &str, data: Value) -> Result<(), String> {
        self.record_entry(event_type, data).map(|_| ())
    }

    fn record_entry(
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

    pub(in crate::agent) fn set_cwd(&mut self, cwd: &Path) -> Result<(), String> {
        self.cwd = cwd.to_path_buf();
        let state_path = self.dir.join("state.json");
        let text = fs::read_to_string(&state_path).map_err(|e| e.to_string())?;
        let mut state: Value = serde_json::from_str(&text)
            .map_err(|e| format!("invalid {}: {}", state_path.display(), e))?;
        let object = state
            .as_object_mut()
            .ok_or_else(|| format!("invalid {}: expected object", state_path.display()))?;
        object.insert("cwd".into(), json!(cwd));
        fs::write(
            &state_path,
            serde_json::to_string_pretty(&state).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| e.to_string())
    }

    pub(in crate::agent) fn artifact_dir(&self) -> PathBuf {
        self.dir.join("artifacts")
    }

    pub(in crate::agent) fn path(&self) -> PathBuf {
        self.dir.clone()
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        path: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "jeden-session-ledger-{name}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn session(&self, name: &str) -> PathBuf {
            let path = self.path.join(name);
            fs::create_dir_all(path.join("artifacts")).unwrap();
            fs::write(
                path.join("state.json"),
                serde_json::to_vec(&json!({
                    "version": crate::cli::sessions::SESSION_LEDGER_VERSION,
                    "id": name,
                    "cwd": self.path,
                    "startedAt": "1",
                    "activeLeaf": null,
                    "lineage": null
                }))
                .unwrap(),
            )
            .unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn recorder_at(dir: PathBuf, cwd: &Path) -> SessionRecorder {
        SessionRecorder {
            id: dir.file_name().unwrap().to_string_lossy().into_owned(),
            dir,
            cwd: cwd.to_path_buf(),
            ready: false,
            active_leaf: None,
            lineage: None,
            pending_tool_results: None,
        }
    }

    fn child_at(
        dir: PathBuf,
        cwd: &Path,
        parent_session: PathBuf,
        parent_entry: String,
    ) -> SessionRecorder {
        let mut recorder = recorder_at(dir, cwd);
        recorder.lineage = Some((parent_session, Some(parent_entry)));
        recorder
    }

    #[test]
    fn session_ledger_legacy_events_roundtrip_through_typed_append_and_replay() {
        let fixture = Fixture::new("legacy-roundtrip");
        let session = fixture.session("legacy");
        fs::write(
            session.join("transcript.jsonl"),
            concat!(
                "{\"ts\":\"1\",\"type\":\"user\",\"data\":{\"task\":\"repair ledger\"}}\n",
                "{\"ts\":\"2\",\"type\":\"final\",\"data\":{\"text\":\"repaired\"}}\n"
            ),
        )
        .unwrap();

        let appended = crate::cli::sessions::append_ledger_entry(
            &session,
            "3".into(),
            "user",
            json!({"task": "verify restart"}),
        )
        .unwrap();
        let value = crate::cli::sessions::read_session_value(session.to_str().unwrap()).unwrap();
        let events = value["events"].as_array().unwrap();

        assert_eq!(events[0]["id"], "legacy-1");
        assert_eq!(events[1]["parentId"], "legacy-1");
        assert_eq!(events[2]["id"], appended.id);
        assert_eq!(events[2]["parentId"], "legacy-2");
        assert!(events
            .iter()
            .all(|event| event["version"] == crate::cli::sessions::SESSION_LEDGER_VERSION));
        assert_eq!(
            crate::cli::sessions::session_conversation_turns(&session).unwrap(),
            vec![
                json!({"role": "user", "content": "repair ledger"}),
                json!({"role": "assistant", "content": "repaired"}),
                json!({"role": "user", "content": "verify restart"}),
            ]
        );
    }

    #[test]
    fn session_ledger_rejects_malformed_middle_instead_of_skipping_history() {
        let fixture = Fixture::new("malformed-middle");
        let session = fixture.session("broken");
        fs::write(
            session.join("transcript.jsonl"),
            concat!(
                "{\"ts\":\"1\",\"type\":\"user\",\"data\":{\"task\":\"before\"}}\n",
                "{not-json}\n",
                "{\"ts\":\"3\",\"type\":\"final\",\"data\":{\"text\":\"after\"}}\n"
            ),
        )
        .unwrap();

        let error = crate::cli::sessions::session_conversation_turns(&session).unwrap_err();
        assert!(
            error.contains("transcript.jsonl:2 is malformed JSON"),
            "{error}"
        );
    }

    #[test]
    fn session_ledger_recovers_complete_entries_before_a_truncated_tail() {
        let fixture = Fixture::new("truncated-tail");
        let session = fixture.session("interrupted");
        fs::write(
            session.join("transcript.jsonl"),
            concat!(
                "{\"ts\":\"1\",\"type\":\"user\",\"data\":{\"task\":\"durable\"}}\n",
                "{\"version\":1,\"id\":\"entry-cut"
            ),
        )
        .unwrap();

        let value = crate::cli::sessions::read_session_value(session.to_str().unwrap()).unwrap();
        assert_eq!(value["recoveredTruncatedTail"], true);
        assert_eq!(value["activeLeaf"], "legacy-1");
        assert_eq!(value["events"].as_array().unwrap().len(), 1);
        assert_eq!(
            crate::cli::sessions::session_conversation_turns(&session).unwrap(),
            vec![json!({"role": "user", "content": "durable"})]
        );
        let error = crate::cli::sessions::append_ledger_entry(
            &session,
            "2".into(),
            "final",
            json!({"text": "must not overwrite tail"}),
        )
        .unwrap_err();
        assert!(error.contains("recovered truncated tail"), "{error}");
    }

    #[test]
    fn session_ledger_replays_a_completed_parallel_tool_result_batch_once() {
        let fixture = Fixture::new("tool-replay");
        let session = fixture.session("tools");
        let mut recorder = recorder_at(session.clone(), &fixture.path);
        recorder.ensure().unwrap();
        recorder
            .record(
                "action",
                json!({"action": {"tools": [{"tool": "read"}, {"tool": "search"}]}}),
            )
            .unwrap();
        recorder
            .record(
                "tool_result",
                json!({"result": {"path": "a.rs", "text": "alpha"}}),
            )
            .unwrap();
        recorder
            .record("tool_result", json!({"result": {"matches": [2, 5]}}))
            .unwrap();
        drop(recorder);

        assert_eq!(
            crate::cli::sessions::session_conversation_turns(&session).unwrap(),
            vec![json!({
                "role": "user",
                "content": "{\"result\":[{\"path\":\"a.rs\",\"text\":\"alpha\"},{\"matches\":[2,5]}],\"type\":\"tool_result\"}"
            })]
        );
    }

    #[test]
    fn session_ledger_compaction_restart_restores_the_compacted_window() {
        let fixture = Fixture::new("compaction-restart");
        let session = fixture.session("compacted");
        let compacted = vec![
            json!({"role": "system", "content": "base policy"}),
            json!({"role": "system", "content": "Prior conversation summary (compacted from 4 messages):\nkeep decision A"}),
        ];
        let mut recorder = recorder_at(session.clone(), &fixture.path);
        recorder.ensure().unwrap();
        recorder
            .record(
                "compaction",
                json!({"before": 4, "summary": "keep decision A"}),
            )
            .unwrap();
        recorder.record_context("compaction", &compacted).unwrap();
        drop(recorder);

        assert_eq!(
            crate::cli::sessions::session_conversation_turns(&session).unwrap(),
            compacted
        );
    }

    #[test]
    fn session_ledger_fork_and_branch_preserve_parent_leaf_and_exact_seed() {
        for reason in ["fork_seed", "branch_seed"] {
            let fixture = Fixture::new(reason);
            let parent = fixture.session("parent");
            let mut parent_recorder = recorder_at(parent.clone(), &fixture.path);
            parent_recorder.ensure().unwrap();
            parent_recorder
                .record("user", json!({"task": format!("seed for {reason}")}))
                .unwrap();
            let parent_leaf = parent_recorder.active_leaf().unwrap().unwrap();
            drop(parent_recorder);

            let child = fixture.path.join("child");
            let seed = vec![
                json!({"role": "system", "content": "policy"}),
                json!({"role": "user", "content": format!("seed for {reason}")}),
            ];
            let mut child_recorder = child_at(
                child.clone(),
                &fixture.path,
                parent.clone(),
                parent_leaf.clone(),
            );
            child_recorder.ensure().unwrap();
            child_recorder.record_context(reason, &seed).unwrap();
            drop(child_recorder);

            let child_value =
                crate::cli::sessions::read_session_value(child.to_str().unwrap()).unwrap();
            assert_eq!(
                child_value["state"]["lineage"]["parentSession"],
                json!(parent)
            );
            assert_eq!(child_value["state"]["lineage"]["parentEntry"], parent_leaf);
            assert_eq!(child_value["events"][0]["type"], "lineage");
            assert_eq!(child_value["events"][0]["data"]["parentEntry"], parent_leaf);
            assert_eq!(
                crate::cli::sessions::session_conversation_turns(&child).unwrap(),
                seed
            );
        }
    }

    #[test]
    fn session_ledger_handoff_child_restart_restores_brief_and_lineage() {
        let fixture = Fixture::new("handoff-restart");
        let parent = fixture.session("parent");
        let mut parent_recorder = recorder_at(parent.clone(), &fixture.path);
        parent_recorder.ensure().unwrap();
        parent_recorder
            .record("handoff", json!({"brief": "continue with migration"}))
            .unwrap();
        let parent_leaf = parent_recorder.active_leaf().unwrap().unwrap();
        drop(parent_recorder);

        let child = fixture.path.join("handoff-child");
        let seed = vec![
            json!({"role": "system", "content": "base policy"}),
            json!({"role": "system", "content": "Handoff brief from the prior session:\ncontinue with migration"}),
        ];
        let mut child_recorder = child_at(
            child.clone(),
            &fixture.path,
            parent.clone(),
            parent_leaf.clone(),
        );
        child_recorder.ensure().unwrap();
        child_recorder
            .record_context("handoff_seed", &seed)
            .unwrap();
        drop(child_recorder);

        let restarted = crate::cli::sessions::read_session_value(child.to_str().unwrap()).unwrap();
        assert_eq!(
            restarted["state"]["lineage"]["parentSession"],
            json!(parent)
        );
        assert_eq!(restarted["state"]["lineage"]["parentEntry"], parent_leaf);
        assert_eq!(
            crate::cli::sessions::session_conversation_turns(&child).unwrap(),
            seed
        );
    }

    struct CountingConsumer {
        deliveries: usize,
        fail_once: bool,
    }

    impl crate::cli::sessions::ledger_v2::SessionOutboxConsumer for CountingConsumer {
        fn consumer(&self) -> crate::cli::sessions::ledger_v2::OutboxConsumer {
            crate::cli::sessions::ledger_v2::OutboxConsumer::Memory
        }

        fn deliver(
            &mut self,
            _event: &crate::cli::sessions::ledger_v2::SessionEventV2,
            idempotency_key: &str,
        ) -> Result<(), String> {
            assert!(idempotency_key.starts_with("session-event:"));
            self.deliveries += 1;
            if std::mem::take(&mut self.fail_once) {
                Err("fixture outage".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn session_outbox_retries_then_delivers_once_across_restart() {
        let fixture = Fixture::new("outbox-restart");
        let session = fixture.session("outbox");
        crate::cli::sessions::append_ledger_entry(
            &session,
            "1".into(),
            "user",
            json!({"task":"durable event"}),
        )
        .unwrap();

        let mut first_process = CountingConsumer {
            deliveries: 0,
            fail_once: true,
        };
        let first =
            crate::cli::sessions::ledger_v2::drain_outbox(&session, &mut first_process, 10, 1)
                .unwrap();
        assert_eq!((first.delivered, first.retried), (0, 1));

        let mut restarted_process = CountingConsumer {
            deliveries: 0,
            fail_once: false,
        };
        let second =
            crate::cli::sessions::ledger_v2::drain_outbox(&session, &mut restarted_process, 11, 10)
                .unwrap();
        assert_eq!((second.delivered, second.retried), (1, 0));
        let third =
            crate::cli::sessions::ledger_v2::drain_outbox(&session, &mut restarted_process, 12, 10)
                .unwrap();
        assert_eq!(third.delivered, 0);
        assert_eq!(restarted_process.deliveries, 1);
    }

    #[test]
    fn session_outbox_expired_lease_is_reclaimed_with_fencing() {
        use crate::cli::sessions::ledger_v2::{outbox, store, OutboxConsumer};
        let fixture = Fixture::new("outbox-lease");
        let session = fixture.session("leased");
        crate::cli::sessions::append_ledger_entry(
            &session,
            "1".into(),
            "user",
            json!({"task":"lease me"}),
        )
        .unwrap();
        let events = store::read_events(&session).unwrap().events;
        let first = outbox::claim(&session, &events, OutboxConsumer::Memory, 10, 30, 12)
            .unwrap()
            .unwrap();
        assert!(
            outbox::claim(&session, &events, OutboxConsumer::Memory, 39, 30, 12)
                .unwrap()
                .is_none()
        );
        let reclaimed = outbox::claim(&session, &events, OutboxConsumer::Memory, 40, 30, 12)
            .unwrap()
            .unwrap();
        assert_eq!(reclaimed.attempt, first.attempt + 1);
        assert!(outbox::complete(&session, &first)
            .unwrap_err()
            .contains("stale outbox lease"));
        outbox::complete(&session, &reclaimed).unwrap();
        assert!(
            outbox::claim(&session, &events, OutboxConsumer::Memory, 100, 30, 12)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn session_ledger_rejects_checksum_tampering() {
        let fixture = Fixture::new("checksum");
        let session = fixture.session("tampered");
        crate::cli::sessions::append_ledger_entry(
            &session,
            "1".into(),
            "user",
            json!({"task":"original"}),
        )
        .unwrap();
        let path = session.join("transcript.jsonl");
        let raw = fs::read_to_string(&path)
            .unwrap()
            .replace("original", "modified");
        fs::write(&path, raw).unwrap();
        let error = crate::cli::sessions::session_conversation_turns(&session).unwrap_err();
        assert!(error.contains("checksum mismatch"), "{error}");
    }
}
