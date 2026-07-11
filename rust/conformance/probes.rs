use super::{
    BehaviorAttempt, BehaviorCheckKind, BehaviorCheckResult, BehaviorEvidence, CheckStatus,
};
use crate::capability::CapabilitySnapshot;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

const CHECK_VERSION: u32 = 2;
const PROTOCOL_VERSION: &str = "jeden.behavior-check.v2";

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
    AreaProbe {
        area: "pelna-macierz-gapow-i-ownership",
        sources: &[source!(
            "registry-contract",
            "rust/conformance/areas.rs",
            [
                "COMPLETION_AREAS",
                "CompletionArea; 38",
                "PRODUCTION_SCOPES"
            ]
        )],
    },
    AreaProbe {
        area: "mierzalne-kryteria-zamkniecia",
        sources: &[source!(
            "computed-gates",
            "rust/conformance/mod.rs",
            ["behavior_complete", "missing_evidence", "computed_complete"]
        )],
    },
    AreaProbe {
        area: "centralny-rejestr-capabilities",
        sources: &[source!(
            "atomic-registry",
            "rust/capability/mod.rs",
            [
                "CapabilityDescriptorV2",
                "CapabilityBinding",
                "build_and_publish"
            ]
        )],
    },
    AreaProbe {
        area: "wersjonowany-graf-sesji",
        sources: &[source!(
            "typed-ledger-migration",
            "rust/agent/runtime/recorder.rs",
            ["legacy_events_roundtrip", "malformed_middle", "active_leaf"]
        )],
    },
    AreaProbe {
        area: "wierne-resume-branch-fork-tree",
        sources: &[source!(
            "lineage-replay",
            "rust/agent/runtime/recorder.rs",
            ["fork_and_branch_preserve", "parent_entry", "active_leaf"]
        )],
    },
    AreaProbe {
        area: "trwale-compaction-handoff-checkpoint-i-rewind",
        sources: &[source!(
            "durable-session-transitions",
            "rust/agent/runtime/recorder.rs",
            ["compaction_restart", "handoff_child_restart", "checkpoint"]
        )],
    },
    AreaProbe {
        area: "operation-context-i-propagowana-cancellation",
        sources: &[source!(
            "operation-token",
            "rust/runtime_ops/mod.rs",
            ["CancellationToken", "OperationContext", "is_cancelled"]
        )],
    },
    AreaProbe {
        area: "process-manager-pty-i-artifact-sink",
        sources: &[
            source!(
                "owned-process-output",
                "rust/runtime_ops/process.rs",
                ["ProcessManager", "ArtifactSink", "cancel"]
            ),
            source!(
                "bounded-output",
                "rust/runtime_ops/output.rs",
                ["BoundedOutput", "ArtifactSink", "truncated"]
            ),
        ],
    },
    AreaProbe {
        area: "dynamiczny-lifecycle-entitlementow-weles",
        sources: &[source!(
            "typed-weles-lifecycle",
            "rust/control_plane/weles.rs",
            [
                "OperationEvent",
                "login_provider",
                "run_operation",
                "logout"
            ]
        )],
    },
    AreaProbe {
        area: "katalog-modeli-i-tras-brama-wisent",
        sources: &[source!(
            "brama-catalog",
            "rust/control_plane/brama.rs",
            ["ModelCatalog", "CachedCatalog", "ttl", "resolve"]
        )],
    },
    AreaProbe {
        area: "typowany-streaming-modelu",
        sources: &[source!(
            "normalized-stream",
            "rust/model_router.rs",
            [
                "StreamingCompletion",
                "StreamErrorClass",
                "RouteResult",
                "visible_output"
            ]
        )],
    },
    AreaProbe {
        area: "retry-failover-i-context-promotion",
        sources: &[source!(
            "retry-router",
            "rust/model_router.rs",
            [
                "RetryPolicy",
                "retry_after",
                "fallbacks",
                "context_promotions"
            ]
        )],
    },
    AreaProbe {
        area: "context-rules-i-secret-policy",
        sources: &[
            source!(
                "context-discovery",
                "rust/context/discovery.rs",
                ["provenance", "max_bytes"]
            ),
            source!(
                "secret-policy",
                "rust/context/secrets.rs",
                ["SecretPolicy", "protect_text"]
            ),
        ],
    },
    AreaProbe {
        area: "unified-read-write-search-resource-semantics",
        sources: &[source!(
            "resource-behavior-fixtures",
            "rust/tool_runtime/tests.rs",
            [
                "ranged_read_of_large_file",
                "recursive_search_honors",
                "sqlite_mutation_requires_current_digest"
            ]
        )],
    },
    AreaProbe {
        area: "ast-i-lsp-runtime",
        sources: &[
            source!(
                "ast-preview-apply",
                "rust/tool_runtime/language/ast.rs",
                ["preview", "apply", "discard"]
            ),
            source!(
                "lsp-lifecycle",
                "rust/tool_runtime/language/lsp.rs",
                ["LspClient", "initialize", "diagnostics", "cancelled"]
            ),
        ],
    },
    AreaProbe {
        area: "persistent-eval-i-terminal-pty",
        sources: &[
            source!(
                "persistent-kernel",
                "rust/runtime_ops/kernel.rs",
                ["KERNELS", "reset", "interrupt"]
            ),
            source!(
                "managed-pty-resize",
                "rust/runtime_ops/pty.rs",
                ["PtyProcess", "resize", "cancelled"]
            ),
        ],
    },
    AreaProbe {
        area: "browser-debugger-web-github-i-ssh",
        sources: &[source!(
            "integration-behavior-fixtures",
            "rust/tool_services/tests.rs",
            [
                "browser_fixture_reuses_session",
                "debugger_fixture_reuses_adapter",
                "github_fixture_covers",
                "ssh_fixture_reuses_control_connection",
                "web_fixture_falls_back"
            ]
        )],
    },
    AreaProbe {
        area: "image-inspect-generate-i-tts",
        sources: &[source!(
            "media-behavior-fixtures",
            "rust/tool_services/tests.rs",
            [
                "media_inspection_success_error_cancel",
                "image_generation_and_tts_fixtures"
            ]
        )],
    },
    AreaProbe {
        area: "pending-actions-checkpoint-resolve-i-rewind",
        sources: &[
            source!(
                "durable-actions",
                "rust/cli/sessions.rs",
                [
                    "PendingActionCreate",
                    "pending_discard",
                    "invalid revision",
                    "expired"
                ]
            ),
            source!(
                "graph-rewind",
                "rust/agent/conversation/history.rs",
                ["checkpoint", "rewind", "parent"]
            ),
        ],
    },
    AreaProbe {
        area: "trwaly-mcp-manager",
        sources: &[source!(
            "persistent-mcp",
            "rust/mcp/mod.rs",
            ["McpManager", "reconnect", "notification", "shutdown"]
        )],
    },
    AreaProbe {
        area: "extension-loader-i-event-bus",
        sources: &[source!(
            "extension-worker",
            "rust/extensions/mod.rs",
            [
                "HostExtension",
                "reload",
                "unhealthy_extensions",
                "execute_tool"
            ]
        )],
    },
    AreaProbe {
        area: "aktywacja-wszystkich-plugin-capabilities",
        sources: &[source!(
            "plugin-activation",
            "rust/extensions/mod.rs",
            [
                "InstalledPluginRoot",
                "capability_descriptors",
                "active",
                "health"
            ]
        )],
    },
    AreaProbe {
        area: "skills-rules-i-custom-agents",
        sources: &[source!(
            "declarative-loader",
            "rust/extensions/declarative.rs",
            ["Skill", "Rule", "Agent", "validate"]
        )],
    },
    AreaProbe {
        area: "task-job-scheduler-i-izolacja",
        sources: &[
            source!(
                "durable-scheduler",
                "rust/task_runtime/scheduler.rs",
                ["TaskScheduler", "cancel", "recover"]
            ),
            source!(
                "workspace-isolation",
                "rust/task_runtime/workspace.rs",
                ["isolation", "merge", "capture"]
            ),
        ],
    },
    AreaProbe {
        area: "agent-communication-i-wspolbieznosc",
        sources: &[source!(
            "durable-mailbox",
            "rust/task_runtime/mailbox.rs",
            ["send", "inbox", "wait", "wake", "correlation"]
        )],
    },
    AreaProbe {
        area: "autonomiczna-pamiec",
        sources: &[source!(
            "memory-worker",
            "rust/memory/mod.rs",
            ["lease", "heartbeat", "consolidat", "provenance"]
        )],
    },
    AreaProbe {
        area: "pelna-live-collaboration",
        sources: &[
            source!(
                "authorized-relay",
                "rust/collab/relay.rs",
                ["write_token", "typed_replay_is_ordered", "since", "cursor"]
            ),
            source!(
                "live-client",
                "rust/collab/client.rs",
                ["LiveClient", "reconnect"]
            ),
        ],
    },
    AreaProbe {
        area: "sdk-rpc-i-acp",
        sources: &[
            source!(
                "public-session-sdk",
                "rust/sdk/session.rs",
                ["AgentSession", "abort", "dispose"]
            ),
            source!(
                "correlated-rpc",
                "rust/rpc/server.rs",
                ["request_id", "abort", "session/event"]
            ),
            source!(
                "acp-adapter",
                "rust/rpc/acp.rs",
                ["AcpBridge", "requestId", "cancel_all"]
            ),
        ],
    },
    AreaProbe {
        area: "odrebny-jezyk-domenowy-i-brand-jeden",
        sources: &[source!(
            "native-identity",
            "rust/main.rs",
            ["jeden", "capabilities", "conformance"]
        )],
    },
    AreaProbe {
        area: "pelny-natywny-editor",
        sources: &[
            source!(
                "editor-behavior-fixtures",
                "rust/tui/editor.rs",
                [
                    "grapheme",
                    "selection_replace_delete_and_undo",
                    "paste_is_sanitized",
                    "ExternalEditor"
                ]
            ),
            source!(
                "managed-external-editor",
                "rust/tui/repl/external_editor.rs",
                [
                    "VISUAL",
                    "EDITOR",
                    "ProcessManager",
                    "inherit_stdio",
                    "external_editor_success_round_trips_unicode",
                    "external_editor_pre_cancel"
                ]
            ),
        ],
    },
    AreaProbe {
        area: "bezpieczny-renderer-i-unicode",
        sources: &[
            source!(
                "unicode-fixtures",
                "rust/tui/text.rs",
                [
                    "sanitizes_terminal_control",
                    "never_split_extended_graphemes"
                ]
            ),
            source!(
                "scrollback-resize",
                "rust/tui/repl/mod.rs",
                ["resize_repaints_only_live_region", "preserves_scrollback"]
            ),
        ],
    },
    AreaProbe {
        area: "attachments-inline-images-i-clipboard",
        sources: &[
            source!(
                "typed-attachments",
                "rust/tui/attachments.rs",
                ["Attachment", "mime", "check_limits", "AttachmentError"]
            ),
            source!(
                "clipboard-provider",
                "rust/slash/session/clipboard.rs",
                ["clipboard_candidates", "Command"]
            ),
        ],
    },
    AreaProbe {
        area: "steering-follow-up-i-konfigurowalne-skroty",
        sources: &[
            source!(
                "delivery-queue",
                "rust/tui/queue.rs",
                ["FollowUpQueue", "DeliveryAction::Steer", "pop_next"]
            ),
            source!(
                "key-conflict-fixture",
                "rust/tui/editor.rs",
                ["rebinding_a_conflicting_chord"]
            ),
        ],
    },
    AreaProbe {
        area: "ui-generowane-z-capability-command-registry",
        sources: &[source!(
            "registry-generated-ui",
            "rust/capability/mod.rs",
            [
                "native_view_descriptors",
                "builtin_slash_descriptors",
                "FunctionTarget"
            ]
        )],
    },
    AreaProbe {
        area: "themes-accessibility-i-live-status",
        sources: &[
            source!(
                "accessible-theme",
                "rust/tui/theme.rs",
                ["NO_COLOR", "Theme"]
            ),
            source!(
                "live-status",
                "rust/tui/integration.rs",
                ["RuntimeStatus", "runtime_status", "route_health"]
            ),
        ],
    },
    AreaProbe {
        area: "doctor-updater-i-subsystem-health",
        sources: &[
            source!(
                "typed-doctor",
                "rust/conformance/health.rs",
                ["DoctorReport", "HealthProbe", "control_plane_probe"]
            ),
            source!(
                "verified-rollback-update",
                "rust/cli/run/slash.rs",
                [
                    "verify_manifest",
                    "checksum mismatch",
                    "previous binary restored",
                    "update_command_rejects_bad_checksum",
                    "update_command_restores_existing_target"
                ]
            ),
        ],
    },
    AreaProbe {
        area: "automatyczny-conformance-reliability-system",
        sources: &[source!(
            "computed-conformance",
            "rust/conformance/mod.rs",
            ["AREA_PROBES", "audit_ui_honesty_paths", "behavior_complete"]
        )],
    },
    AreaProbe {
        area: "usuniecie-ui-only-no-op-dead-paths",
        sources: &[source!(
            "ui-honesty-gate",
            "rust/conformance/mod.rs",
            [
                "descriptor-handler-health-surface",
                "executable-without-health",
                "audit_ui_honesty_paths"
            ]
        )],
    },
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceArtifact {
    protocol_version: String,
    check_version: u32,
    fixture_digest: String,
    command_or_scenario_id: String,
    started_at: u64,
    finished_at: u64,
    expires_at: u64,
    attempts: Vec<BehaviorAttempt>,
    outcome: String,
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn inventory_check(root: &Path, area: &str, probe: &SourceProbe) -> BehaviorCheckResult {
    let path = root.join(probe.path);
    let failure = match fs::read_to_string(&path) {
        Ok(source) => probe
            .symbols
            .iter()
            .find(|symbol| !source.contains(**symbol))
            .map(|symbol| format!("missing required symbol `{symbol}` in {}", path.display())),
        Err(error) => Some(format!("cannot read {}: {error}", path.display())),
    };
    BehaviorCheckResult {
        id: format!("{area}/{}/inventory", probe.id),
        kind: BehaviorCheckKind::Inventory,
        check_version: CHECK_VERSION,
        status: if failure.is_some() {
            CheckStatus::Failed
        } else {
            CheckStatus::NotRun
        },
        fixture_digest: None,
        command_or_scenario_id: None,
        started_at: None,
        finished_at: None,
        attempts: Vec::new(),
        evidence_artifact_digest: None,
        protocol_version: PROTOCOL_VERSION.into(),
        detail: failure.unwrap_or_else(|| {
            format!(
                "inventory anchors found in {}; source presence is not behavioral evidence",
                probe.path
            )
        }),
    }
}

fn evidence_check(
    root: &Path,
    area: &str,
    probe: &SourceProbe,
    now_ms: u64,
) -> (
    BehaviorCheckResult,
    Option<BehaviorEvidence>,
    Option<String>,
) {
    let id = format!("{area}/{}/behavior", probe.id);
    let relative = format!(".jeden/conformance/evidence/{area}--{}.json", probe.id);
    let path = root.join(&relative);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return (
                BehaviorCheckResult::not_run(
                    id,
                    format!("missing behavioral evidence {}: {error}", path.display()),
                ),
                None,
                Some(probe.id.into()),
            )
        }
    };
    let artifact_digest = hex::encode(Sha256::digest(&bytes));
    let artifact: EvidenceArtifact = match serde_json::from_slice(&bytes) {
        Ok(artifact) => artifact,
        Err(error) => {
            return (
                BehaviorCheckResult::failed(
                    id,
                    format!("invalid behavioral evidence {}: {error}", path.display()),
                ),
                None,
                Some(probe.id.into()),
            )
        }
    };
    let external_blocked = artifact.outcome == "external-blocked";
    let invalid = if artifact.protocol_version != PROTOCOL_VERSION {
        Some(format!(
            "unsupported protocol version {}",
            artifact.protocol_version
        ))
    } else if artifact.check_version != CHECK_VERSION {
        Some(format!(
            "unsupported check version {}",
            artifact.check_version
        ))
    } else if !valid_digest(&artifact.fixture_digest) {
        Some("fixture digest must be a SHA-256 hex digest".into())
    } else if artifact.command_or_scenario_id.trim().is_empty() {
        Some("command/scenario ID is empty".into())
    } else if artifact.attempts.is_empty() {
        Some("evidence contains no execution attempts".into())
    } else if artifact.started_at > artifact.finished_at
        || artifact.finished_at > artifact.expires_at
    {
        Some("evidence timestamps are inconsistent".into())
    } else if artifact.expires_at < now_ms {
        Some(format!(
            "behavioral evidence expired at {}",
            artifact.expires_at
        ))
    } else if external_blocked {
        Some("behavioral scenario is blocked by an explicit external prerequisite".into())
    } else if artifact.outcome != "passed" {
        Some(format!(
            "behavioral scenario outcome is {}",
            artifact.outcome
        ))
    } else if artifact
        .attempts
        .iter()
        .any(|attempt| attempt.outcome != "passed" || attempt.started_at > attempt.finished_at)
    {
        Some("one or more execution attempts failed or have inconsistent timestamps".into())
    } else {
        None
    };
    let result = BehaviorCheckResult {
        id,
        kind: BehaviorCheckKind::Behavior,
        check_version: artifact.check_version,
        status: if external_blocked {
            CheckStatus::ExternalBlocked
        } else if invalid.is_some() {
            CheckStatus::Failed
        } else {
            CheckStatus::Passed
        },
        fixture_digest: Some(artifact.fixture_digest.clone()),
        command_or_scenario_id: Some(artifact.command_or_scenario_id.clone()),
        started_at: Some(artifact.started_at),
        finished_at: Some(artifact.finished_at),
        attempts: artifact.attempts,
        evidence_artifact_digest: Some(artifact_digest.clone()),
        protocol_version: artifact.protocol_version,
        detail: invalid
            .clone()
            .unwrap_or_else(|| "executable behavioral evidence is valid and fresh".into()),
    };
    if let Some(reason) = invalid {
        return (result, None, Some(format!("{}: {reason}", probe.id)));
    }
    let evidence = BehaviorEvidence {
        area: area.into(),
        scenario: probe.id.into(),
        outcome: "verified".into(),
        artifact: Some(relative),
        artifact_digest: Some(artifact_digest),
    };
    (result, Some(evidence), None)
}

pub(crate) fn evaluate(
    root: &Path,
    area: &str,
    _snapshot: &CapabilitySnapshot,
    now_ms: u64,
) -> (Vec<BehaviorCheckResult>, Vec<BehaviorEvidence>, Vec<String>) {
    let Some(spec) = AREA_PROBES.iter().find(|probe| probe.area == area) else {
        return (
            vec![BehaviorCheckResult::failed(
                format!("{area}/probe-registration"),
                "no conformance probe is registered",
            )],
            Vec::new(),
            vec!["unregistered-area-probe".into()],
        );
    };
    let mut checks = Vec::new();
    let mut evidence = Vec::new();
    let mut missing = Vec::new();
    for probe in spec.sources {
        checks.push(inventory_check(root, area, probe));
        let (result, item, absent) = evidence_check(root, area, probe, now_ms);
        checks.push(result);
        if let Some(item) = item {
            evidence.push(item);
        }
        if let Some(absent) = absent {
            missing.push(absent);
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct TempRoot(std::path::PathBuf);
    impl TempRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jeden-behavior-contract-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn evidence(&self, outcome: &str, expires_at: u64) {
            let directory = self.0.join(".jeden/conformance/evidence");
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("pelna-macierz-gapow-i-ownership--registry-contract.json"), format!(r#"{{"protocolVersion":"jeden.behavior-check.v2","checkVersion":2,"fixtureDigest":"{}","commandOrScenarioId":"negative-symbol-fixture","startedAt":10,"finishedAt":20,"expiresAt":{},"attempts":[{{"attempt":1,"startedAt":10,"finishedAt":20,"outcome":"{}","detail":"fixture execution"}}],"outcome":"{}"}}"#, "a".repeat(64), expires_at, outcome, outcome)).unwrap();
        }
        fn source(&self) {
            let path = self.0.join("rust/conformance/areas.rs");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                path,
                "COMPLETION_AREAS CompletionArea; 38 PRODUCTION_SCOPES",
            )
            .unwrap();
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn present_symbols_with_broken_behavior_fail() {
        let root = TempRoot::new();
        root.source();
        root.evidence("failed", 100);
        let (checks, evidence, missing) = evaluate(
            &root.0,
            "pelna-macierz-gapow-i-ownership",
            &crate::capability::snapshot(),
            50,
        );
        assert!(checks
            .iter()
            .any(|check| check.kind == BehaviorCheckKind::Inventory
                && check.status == CheckStatus::NotRun));
        assert!(checks
            .iter()
            .any(|check| check.kind == BehaviorCheckKind::Behavior
                && check.status == CheckStatus::Failed));
        assert!(evidence.is_empty());
        assert!(!missing.is_empty());
    }

    #[test]
    fn missing_and_stale_evidence_fail_closed() {
        let root = TempRoot::new();
        root.source();
        let (missing_checks, _, missing) = evaluate(
            &root.0,
            "pelna-macierz-gapow-i-ownership",
            &crate::capability::snapshot(),
            50,
        );
        assert!(missing_checks
            .iter()
            .any(|check| check.kind == BehaviorCheckKind::Behavior
                && check.status == CheckStatus::NotRun));
        assert!(!missing.is_empty());
        root.evidence("passed", 40);
        let (stale_checks, _, stale) = evaluate(
            &root.0,
            "pelna-macierz-gapow-i-ownership",
            &crate::capability::snapshot(),
            50,
        );
        assert!(stale_checks
            .iter()
            .any(|check| check.kind == BehaviorCheckKind::Behavior
                && check.status == CheckStatus::Failed
                && check.detail.contains("expired")));
        assert!(!stale.is_empty());
    }
}
