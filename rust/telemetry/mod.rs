mod export;
mod privacy;
mod recorder;
mod schema;

pub use export::{ExportError, ExportStatus, OtlpExporter};
pub use privacy::{contains_canary, PrivacyFilter};
pub use recorder::{
    CaptureStatus, RetentionReport, TelemetryConfig, TelemetryHealth, TelemetryRecorder,
};
pub use schema::{
    AuditAction, AuditDecision, AuditEvent, CorrelationIds, MetricEvent, MetricName, PrivateId,
    TelemetryEnvelope, TelemetryRecord, TerminalState, TraceEvent, TraceKind, TracePhase,
    ALLOWLISTED_FIELDS,
};

use std::sync::Arc;

/// Adapter implemented by OperationContextV2 after its security-owned shape is finalized.
/// Keeping this trait here avoids a competing operation context or authority-bearing clone.
pub trait TelemetryContextAdapter {
    fn telemetry(&self) -> Option<&TelemetryHandle>;
}

/// Narrow operation-scoped facade: callers can emit only closed-schema events, never arbitrary
/// labels, messages, prompts, paths, URLs, hosts, command arguments, or tool I/O.
#[derive(Clone)]
pub struct TelemetryHandle {
    recorder: Arc<TelemetryRecorder>,
    privacy: PrivacyFilter,
    ids: CorrelationIds,
}

impl std::fmt::Debug for TelemetryHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TelemetryHandle")
            .field("ids", &self.ids)
            .finish_non_exhaustive()
    }
}

impl TelemetryHandle {
    pub fn new(
        recorder: Arc<TelemetryRecorder>,
        privacy: PrivacyFilter,
        ids: CorrelationIds,
    ) -> Self {
        Self {
            recorder,
            privacy,
            ids,
        }
    }

    pub fn ids(&self) -> &CorrelationIds {
        &self.ids
    }

    pub fn child(&self, raw_operation_id: &str) -> Self {
        let mut ids = self.ids.clone();
        ids.parent_operation_id = Some(self.ids.operation_id.clone());
        ids.operation_id = self.privacy.pseudonymize("operation", raw_operation_id);
        ids.attempt_id = None;
        Self {
            recorder: self.recorder.clone(),
            privacy: self.privacy.clone(),
            ids,
        }
    }

    pub fn trace(
        &self,
        kind: TraceKind,
        phase: TracePhase,
        terminal_state: Option<TerminalState>,
        duration_micros: Option<u64>,
    ) -> CaptureStatus {
        self.recorder.capture(TelemetryRecord::Trace(TraceEvent {
            kind,
            phase,
            ids: self.ids.clone(),
            terminal_state,
            duration_micros,
        }))
    }

    pub fn metric(&self, name: MetricName, value: u64) -> CaptureStatus {
        self.recorder.capture(TelemetryRecord::Metric(MetricEvent {
            name,
            value,
            ids: Some(self.ids.clone()),
        }))
    }

    pub fn audit(
        &self,
        action: AuditAction,
        decision: AuditDecision,
        raw_subject_id: &str,
    ) -> CaptureStatus {
        self.recorder.capture(TelemetryRecord::Audit(AuditEvent {
            action,
            decision,
            subject_id: self.privacy.pseudonymize("audit-subject", raw_subject_id),
            ids: self.ids.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use super::*;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "jeden-private-telemetry-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).expect("create fixture");
            Self { root }
        }

        fn config(&self, max_records: usize, max_bytes: usize) -> TelemetryConfig {
            TelemetryConfig {
                spool_path: self.root.join("spool.jsonl"),
                audit_path: self.root.join("security-audit.jsonl"),
                max_records,
                max_bytes,
                retention: Duration::from_secs(60),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn handle(recorder: Arc<TelemetryRecorder>, canaries: &[&str]) -> TelemetryHandle {
        let privacy = PrivacyFilter::new([23; 32]);
        let ids = privacy.correlation_ids(
            canaries[0],
            canaries[1],
            Some(canaries[2]),
            Some(canaries[3]),
            Some(canaries[4]),
            Some(canaries[5]),
            Some(17),
        );
        TelemetryHandle::new(recorder, privacy, ids)
    }

    #[test]
    fn focused_capture_is_allowlisted_bounded_and_canary_free() {
        let fixture = Fixture::new("capture");
        let recorder = Arc::new(TelemetryRecorder::local_only(fixture.config(3, 64 * 1024)));
        let canaries = [
            "PROMPT_CANARY_do_not_store",
            "/private/repo/PATH_CANARY",
            "https://HOST_CANARY.example/path?TOKEN_CANARY=yes",
            "--password=ARGS_CANARY",
            "RAW_TOOL_INPUT_CANARY",
            "RAW_TOOL_OUTPUT_CANARY",
            "SECRET_NAME_CANARY",
        ];
        let telemetry = handle(recorder.clone(), &canaries);
        assert_eq!(
            telemetry.trace(TraceKind::Operation, TracePhase::Started, None, None),
            CaptureStatus::Captured
        );
        assert_eq!(
            telemetry.metric(MetricName::OperationStarted, 1),
            CaptureStatus::Captured
        );
        assert_eq!(
            telemetry.audit(AuditAction::GrantDenied, AuditDecision::Denied, canaries[6]),
            CaptureStatus::Captured
        );
        assert_eq!(
            telemetry.metric(MetricName::Retry, 1),
            CaptureStatus::DroppedFull
        );
        assert_eq!(recorder.health().dropped_records, 1);
        assert_eq!(recorder.flush_local(), 3);

        let mut captured = fs::read(&fixture.config(3, 64 * 1024).spool_path).expect("spool");
        captured.extend(fs::read(&fixture.config(3, 64 * 1024).audit_path).expect("audit"));
        assert!(!contains_canary(&captured, &canaries));

        for line in String::from_utf8(captured).expect("utf8").lines() {
            let value: serde_json::Value = serde_json::from_str(line).expect("typed json");
            assert_allowlisted(&value);
        }
    }

    fn assert_allowlisted(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    assert!(
                        ALLOWLISTED_FIELDS.contains(&key.as_str()),
                        "unexpected field: {key}"
                    );
                    assert_allowlisted(child);
                }
            }
            serde_json::Value::Array(values) => values.iter().for_each(assert_allowlisted),
            _ => {}
        }
    }

    struct CountingExporter {
        calls: AtomicUsize,
        fail: bool,
    }

    impl OtlpExporter for CountingExporter {
        fn export(&self, _batch: &[TelemetryEnvelope]) -> Result<(), ExportError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                Err(ExportError::Unavailable)
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn opt_out_has_no_export_path_and_export_failure_is_non_fatal() {
        let fixture = Fixture::new("export");
        let local = Arc::new(TelemetryRecorder::local_only(fixture.config(8, 64 * 1024)));
        let telemetry = handle(
            local.clone(),
            &["op", "session", "parent", "attempt", "route", "capability"],
        );
        assert_eq!(
            telemetry.metric(MetricName::OperationStarted, 1),
            CaptureStatus::Captured
        );
        assert_eq!(local.export_pending(), ExportStatus::Disabled);

        let exporter = Arc::new(CountingExporter {
            calls: AtomicUsize::new(0),
            fail: true,
        });
        let opted_in = Arc::new(TelemetryRecorder::with_otlp_exporter(
            fixture.config(8, 64 * 1024),
            exporter.clone(),
        ));
        let telemetry = handle(
            opted_in.clone(),
            &["op", "session", "parent", "attempt", "route", "capability"],
        );
        assert_eq!(
            telemetry.metric(MetricName::OperationStarted, 1),
            CaptureStatus::Captured
        );
        assert_eq!(
            opted_in.export_pending(),
            ExportStatus::Failed(ExportError::Unavailable)
        );
        assert_eq!(exporter.calls.load(Ordering::Relaxed), 1);
        assert_eq!(opted_in.health().export_failures, 1);
        assert_eq!(opted_in.health().queued_records, 1);
    }

    #[test]
    fn child_handle_preserves_session_and_sets_operation_parentage() {
        let fixture = Fixture::new("parentage");
        let recorder = Arc::new(TelemetryRecorder::local_only(fixture.config(8, 64 * 1024)));
        let parent = handle(
            recorder,
            &[
                "parent-op",
                "session",
                "prior",
                "attempt",
                "route",
                "capability",
            ],
        );
        let child = parent.child("child-op");
        assert_eq!(child.ids().session_id, parent.ids().session_id);
        assert_eq!(
            child.ids().parent_operation_id.as_ref(),
            Some(&parent.ids().operation_id)
        );
        assert_ne!(child.ids().operation_id, parent.ids().operation_id);
        assert_eq!(child.ids().attempt_id, None);
    }

    #[test]
    fn retention_and_session_delete_cover_spool_and_audit() {
        let fixture = Fixture::new("retention");
        let recorder = Arc::new(TelemetryRecorder::local_only(fixture.config(16, 64 * 1024)));
        let first = handle(
            recorder.clone(),
            &[
                "op-a",
                "session-a",
                "parent",
                "attempt",
                "route",
                "capability",
            ],
        );
        let second = handle(
            recorder.clone(),
            &[
                "op-b",
                "session-b",
                "parent",
                "attempt",
                "route",
                "capability",
            ],
        );
        first.trace(
            TraceKind::Operation,
            TracePhase::Completed,
            Some(TerminalState::Succeeded),
            Some(7),
        );
        first.audit(
            AuditAction::ApprovalGranted,
            AuditDecision::Allowed,
            "principal-a",
        );
        second.metric(MetricName::OperationCompleted, 1);
        assert_eq!(recorder.flush_local(), 3);

        let deleted = recorder.delete_session(&first.ids().session_id);
        assert_eq!(deleted.deleted_session_records, 2);
        let spool = fs::read_to_string(&fixture.config(16, 64 * 1024).spool_path).expect("spool");
        assert!(!spool.contains(first.ids().session_id.as_str()));
        assert!(spool.contains(second.ids().session_id.as_str()));
        let audit = fs::read_to_string(&fixture.config(16, 64 * 1024).audit_path).expect("audit");
        assert!(!audit.contains(first.ids().session_id.as_str()));

        let retained = recorder.enforce_retention(SystemTime::now() + Duration::from_secs(120));
        assert_eq!(retained.expired_records, 1);
        assert!(
            fs::read_to_string(&fixture.config(16, 64 * 1024).spool_path)
                .expect("spool")
                .is_empty()
        );
    }
}
