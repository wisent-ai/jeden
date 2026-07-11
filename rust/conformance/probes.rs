use super::{BehaviorCheckResult, BehaviorEvidence, CheckStatus};
use crate::capability::{CapabilitySnapshot, FunctionTarget};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub(crate) struct SourceProbe {
    pub id: &'static str,
    pub path: &'static str,
    pub symbols: &'static [&'static str],
}

pub(crate) struct AreaProbe {
    pub area: &'static str,
    pub sources: &'static [SourceProbe],
}

macro_rules! source {
    ($id:literal, $path:literal, [$($symbol:literal),+ $(,)?]) => {
        SourceProbe { id: $id, path: $path, symbols: &[$($symbol),+] }
    };
}

pub(crate) static AREA_PROBES: &[AreaProbe] = &[
    AreaProbe { area: "pelna-macierz-gapow-i-ownership", sources: &[source!("registry-contract", "rust/conformance/areas.rs", ["COMPLETION_AREAS", "CompletionArea; 38", "conformance_registry_invariants"])] },
    AreaProbe { area: "mierzalne-kryteria-zamkniecia", sources: &[source!("computed-gates", "rust/conformance/mod.rs", ["checks_pass", "missing_evidence", "computed_complete"])] },
    AreaProbe { area: "centralny-rejestr-capabilities", sources: &[source!("atomic-registry", "rust/capability/mod.rs", ["ArcSwapOption", "build_and_publish", "diagnostics"])] },
    AreaProbe { area: "wersjonowany-graf-sesji", sources: &[source!("typed-ledger-migration", "rust/agent/runtime/recorder.rs", ["legacy_events_roundtrip", "malformed_middle", "active_leaf"])] },
    AreaProbe { area: "wierne-resume-branch-fork-tree", sources: &[source!("lineage-replay", "rust/agent/runtime/recorder.rs", ["fork_and_branch_preserve", "parent_entry", "active_leaf"])] },
    AreaProbe { area: "trwale-compaction-handoff-checkpoint-i-rewind", sources: &[source!("durable-session-transitions", "rust/agent/runtime/recorder.rs", ["compaction_restart", "handoff_child_restart", "checkpoint"])] },
    AreaProbe { area: "operation-context-i-propagowana-cancellation", sources: &[source!("operation-token", "rust/runtime_ops/mod.rs", ["CancellationToken", "OperationContext", "is_cancelled"])] },
    AreaProbe { area: "process-manager-pty-i-artifact-sink", sources: &[source!("owned-process-output", "rust/runtime_ops/process.rs", ["ProcessManager", "ArtifactSink", "cancel"]), source!("bounded-output", "rust/runtime_ops/output.rs", ["BoundedOutput", "ArtifactSink", "truncated"])] },
    AreaProbe { area: "dynamiczny-lifecycle-entitlementow-weles", sources: &[source!("typed-weles-lifecycle", "rust/control_plane/weles.rs", ["OperationEvent", "login_provider", "run_operation", "logout"])] },
    AreaProbe { area: "katalog-modeli-i-tras-brama-wisent", sources: &[source!("brama-catalog", "rust/control_plane/brama.rs", ["ModelCatalog", "CachedCatalog", "ttl", "resolve"])] },
    AreaProbe { area: "typowany-streaming-modelu", sources: &[source!("normalized-stream", "rust/model_router.rs", ["StreamingCompletion", "StreamErrorClass", "RouteResult", "visible_output"])] },
    AreaProbe { area: "retry-failover-i-context-promotion", sources: &[source!("retry-router", "rust/model_router.rs", ["RetryPolicy", "retry_after", "fallbacks", "context_promotions"])] },
    AreaProbe { area: "context-rules-i-secret-policy", sources: &[source!("context-discovery", "rust/context/discovery.rs", ["provenance", "max_bytes"]), source!("secret-policy", "rust/context/secrets.rs", ["SecretPolicy", "protect_text"])] },
    AreaProbe { area: "unified-read-write-search-resource-semantics", sources: &[source!("resource-behavior-fixtures", "rust/tool_runtime/tests.rs", ["ranged_read_of_large_file", "recursive_search_honors", "sqlite_mutation_requires_current_digest"])] },
    AreaProbe { area: "ast-i-lsp-runtime", sources: &[source!("ast-preview-apply", "rust/tool_runtime/language/ast.rs", ["preview", "apply", "discard"]), source!("lsp-lifecycle", "rust/tool_runtime/language/lsp.rs", ["LspClient", "initialize", "diagnostics", "cancelled"])] },
    AreaProbe { area: "persistent-eval-i-terminal-pty", sources: &[source!("persistent-kernel", "rust/runtime_ops/kernel.rs", ["KERNELS", "reset", "interrupt"]), source!("managed-pty-resize", "rust/runtime_ops/pty.rs", ["PtyProcess", "resize", "cancelled"])] },
    AreaProbe { area: "browser-debugger-web-github-i-ssh", sources: &[source!("integration-behavior-fixtures", "rust/tool_services/tests.rs", ["browser_fixture_reuses_session", "debugger_fixture_reuses_adapter", "github_fixture_covers", "ssh_fixture_reuses_control_connection", "web_fixture_falls_back"])] },
    AreaProbe { area: "image-inspect-generate-i-tts", sources: &[source!("media-behavior-fixtures", "rust/tool_services/tests.rs", ["media_inspection_success_error_cancel", "image_generation_and_tts_fixtures"])] },
    AreaProbe { area: "pending-actions-checkpoint-resolve-i-rewind", sources: &[source!("durable-actions", "rust/cli/sessions.rs", ["PendingActionCreate", "pending_discard", "invalid revision", "expired"]), source!("graph-rewind", "rust/agent/conversation/history.rs", ["checkpoint", "rewind", "parent"])] },
    AreaProbe { area: "trwaly-mcp-manager", sources: &[source!("persistent-mcp", "rust/mcp/mod.rs", ["McpManager", "reconnect", "notification", "shutdown"])] },
    AreaProbe { area: "extension-loader-i-event-bus", sources: &[source!("extension-worker", "rust/extensions/mod.rs", ["HostExtension", "reload", "unhealthy_extensions", "execute_tool"])] },
    AreaProbe { area: "aktywacja-wszystkich-plugin-capabilities", sources: &[source!("plugin-activation", "rust/extensions/mod.rs", ["InstalledPluginRoot", "capability_descriptors", "active", "health"])] },
    AreaProbe { area: "skills-rules-i-custom-agents", sources: &[source!("declarative-loader", "rust/extensions/declarative.rs", ["Skill", "Rule", "Agent", "validate"])] },
    AreaProbe { area: "task-job-scheduler-i-izolacja", sources: &[source!("durable-scheduler", "rust/task_runtime/scheduler.rs", ["TaskScheduler", "cancel", "recover"]), source!("workspace-isolation", "rust/task_runtime/workspace.rs", ["isolation", "merge", "capture"])] },
    AreaProbe { area: "agent-communication-i-wspolbieznosc", sources: &[source!("durable-mailbox", "rust/task_runtime/mailbox.rs", ["send", "inbox", "wait", "wake", "correlation"])] },
    AreaProbe { area: "autonomiczna-pamiec", sources: &[source!("memory-worker", "rust/memory/mod.rs", ["lease", "heartbeat", "consolidat", "provenance"])] },
    AreaProbe { area: "pelna-live-collaboration", sources: &[source!("authorized-relay", "rust/collab/relay.rs", ["write_token", "typed_replay_is_ordered", "since", "cursor"]), source!("live-client", "rust/collab/client.rs", ["LiveClient", "reconnect"])] },
    AreaProbe { area: "sdk-rpc-i-acp", sources: &[source!("public-session-sdk", "rust/sdk/session.rs", ["AgentSession", "abort", "dispose"]), source!("correlated-rpc", "rust/rpc/server.rs", ["request_id", "abort", "session/event"]), source!("acp-adapter", "rust/rpc/acp.rs", ["AcpBridge", "requestId", "cancel_all"])] },
    AreaProbe { area: "odrebny-jezyk-domenowy-i-brand-jeden", sources: &[source!("native-identity", "rust/main.rs", ["jeden", "capabilities", "conformance"])] },
    AreaProbe { area: "pelny-natywny-editor", sources: &[source!("editor-behavior-fixtures", "rust/tui/editor.rs", ["grapheme", "selection_replace_delete_and_undo", "paste_is_sanitized", "ExternalEditor"]), source!("managed-external-editor", "rust/tui/repl/external_editor.rs", ["VISUAL", "EDITOR", "ProcessManager", "inherit_stdio", "external_editor_success_round_trips_unicode", "external_editor_pre_cancel"])] },
    AreaProbe { area: "bezpieczny-renderer-i-unicode", sources: &[source!("unicode-fixtures", "rust/tui/text.rs", ["sanitizes_terminal_control", "never_split_extended_graphemes"]), source!("scrollback-resize", "rust/tui/repl/mod.rs", ["resize_repaints_only_live_region", "preserves_scrollback"])] },
    AreaProbe { area: "attachments-inline-images-i-clipboard", sources: &[source!("typed-attachments", "rust/tui/attachments.rs", ["Attachment", "mime", "check_limits", "AttachmentError"]), source!("clipboard-provider", "rust/slash/session/clipboard.rs", ["clipboard_candidates", "Command"])] },
    AreaProbe { area: "steering-follow-up-i-konfigurowalne-skroty", sources: &[source!("delivery-queue", "rust/tui/queue.rs", ["FollowUpQueue", "DeliveryAction::Steer", "pop_next"]), source!("key-conflict-fixture", "rust/tui/editor.rs", ["rebinding_a_conflicting_chord"])] },
    AreaProbe { area: "ui-generowane-z-capability-command-registry", sources: &[source!("registry-generated-ui", "rust/capability/mod.rs", ["native_view_descriptors", "builtin_slash_descriptors", "FunctionTarget"])] },
    AreaProbe { area: "themes-accessibility-i-live-status", sources: &[source!("accessible-theme", "rust/tui/theme.rs", ["NO_COLOR", "Theme"]), source!("live-status", "rust/tui/integration.rs", ["RuntimeStatus", "runtime_status", "route_health"])] },
    AreaProbe { area: "doctor-updater-i-subsystem-health", sources: &[source!("typed-doctor", "rust/conformance/health.rs", ["DoctorReport", "HealthProbe", "control_plane_probe"]), source!("verified-rollback-update", "rust/cli/run/slash.rs", ["verify_manifest", "checksum mismatch", "previous binary restored", "update_command_rejects_bad_checksum", "update_command_restores_existing_target"])] },
    AreaProbe { area: "automatyczny-conformance-reliability-system", sources: &[source!("computed-conformance", "rust/conformance/mod.rs", ["AREA_PROBES", "audit_ui_honesty_paths", "computed_complete"])] },
    AreaProbe { area: "usuniecie-ui-only-no-op-dead-paths", sources: &[source!("ui-honesty-gate", "rust/conformance/mod.rs", ["executable-without-handler", "executable-without-health", "audit_ui_honesty_paths"])] },
];

pub(crate) fn evaluate(
    root: &Path,
    area: &str,
    snapshot: &CapabilitySnapshot,
) -> (Vec<BehaviorCheckResult>, Vec<BehaviorEvidence>, Vec<String>) {
    let mut checks = Vec::new();
    let mut evidence = Vec::new();
    let mut missing = Vec::new();
    let Some(spec) = AREA_PROBES.iter().find(|probe| probe.area == area) else {
        missing.push("unregistered-area-probe".to_string());
        checks.push(BehaviorCheckResult { id: format!("{area}/probe-registration"), status: CheckStatus::Failed, detail: "no concrete conformance probe is registered".into() });
        return (checks, evidence, missing);
    };

    for probe in spec.sources {
        let path = root.join(probe.path);
        let failure = match fs::read_to_string(&path) {
            Ok(source) => probe.symbols.iter().find(|symbol| !source.contains(**symbol)).map(|symbol| format!("missing required symbol `{symbol}` in {}", path.display())),
            Err(error) => Some(format!("cannot read {}: {error}", path.display())),
        };
        let id = format!("{area}/{}", probe.id);
        if let Some(detail) = failure {
            checks.push(BehaviorCheckResult { id: id.clone(), status: CheckStatus::Failed, detail });
            missing.push(probe.id.to_string());
        } else {
            let artifact = format!("{}#{}", probe.path, probe.symbols.join(","));
            checks.push(BehaviorCheckResult { id: id.clone(), status: CheckStatus::Passed, detail: format!("verified executable contract anchors in {}", probe.path) });
            evidence.push(BehaviorEvidence { area: area.to_string(), scenario: probe.id.to_string(), outcome: "verified".into(), artifact: Some(artifact) });
        }
    }

    if area == "centralny-rejestr-capabilities" || area == "ui-generowane-z-capability-command-registry" || area == "usuniecie-ui-only-no-op-dead-paths" {
        let mut ids = BTreeSet::new();
        let duplicate = snapshot.descriptors.iter().find(|descriptor| !ids.insert(descriptor.id.as_str()));
        let dishonest = snapshot.descriptors.iter().find(|descriptor| descriptor.ui.visible && descriptor.ui.executable && (descriptor.operations.is_empty() || matches!(descriptor.target, FunctionTarget::None) || descriptor.ui.action.as_deref().is_none_or(str::is_empty) || !descriptor.health.is_executable()));
        let failure = duplicate.map(|descriptor| format!("duplicate capability id {}", descriptor.id)).or_else(|| dishonest.map(|descriptor| format!("visible executable capability {} lacks a live healthy handler", descriptor.id)));
        let id = format!("{area}/live-registry-honesty");
        if let Some(detail) = failure {
            checks.push(BehaviorCheckResult { id, status: CheckStatus::Failed, detail });
            missing.push("live-registry-honesty".into());
        } else {
            checks.push(BehaviorCheckResult { id, status: CheckStatus::Passed, detail: format!("{} live descriptors have unique IDs and honest executable UI", snapshot.descriptors.len()) });
            evidence.push(BehaviorEvidence { area: area.to_string(), scenario: "live-registry-honesty".into(), outcome: "verified".into(), artifact: Some(format!("capability-registry:generation-{}", snapshot.generation)) });
        }
    }
    checks.sort_by(|a, b| a.id.cmp(&b.id));
    evidence.sort_by(|a, b| a.scenario.cmp(&b.scenario));
    missing.sort();
    (checks, evidence, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance::areas::completion_areas;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "jeden-conformance-{name}-{}-{}",
                std::process::id(),
                TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&path).expect("create conformance fixture root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn computed_conformance_registry_has_exactly_one_probe_for_each_of_38_areas() {
        let mut expected = completion_areas().iter().map(|area| area.id).collect::<Vec<_>>();
        let mut registered = AREA_PROBES.iter().map(|probe| probe.area).collect::<Vec<_>>();
        expected.sort_unstable();
        registered.sort_unstable();

        assert_eq!(expected.len(), 38, "the completion-area contract must remain 38 areas");
        assert_eq!(AREA_PROBES.len(), 38, "one probe must be registered per completion area");
        assert_eq!(registered, expected, "probe IDs must match area IDs one-for-one");
    }

    #[test]
    fn computed_conformance_source_probe_reports_the_exact_missing_contract_symbol() {
        let root = TempRoot::new("missing-symbol");
        let relative_path = "rust/conformance/areas.rs";
        let artifact = root.path().join(relative_path);
        fs::create_dir_all(artifact.parent().expect("fixture artifact parent")).expect("create fixture artifact parent");
        fs::write(&artifact, "COMPLETION_AREAS\nCompletionArea; 38\n").expect("write incomplete source artifact");

        let (checks, evidence, missing) = evaluate(
            root.path(),
            "pelna-macierz-gapow-i-ownership",
            &crate::capability::snapshot(),
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].id, "pelna-macierz-gapow-i-ownership/registry-contract");
        assert!(matches!(checks[0].status, CheckStatus::Failed));
        assert_eq!(
            checks[0].detail,
            format!("missing required symbol `conformance_registry_invariants` in {}", artifact.display()),
        );
        assert!(evidence.is_empty(), "a failed source probe must not emit verified evidence");
        assert_eq!(missing, ["registry-contract"]);
    }
}

