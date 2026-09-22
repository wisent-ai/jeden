//! The second half of the conformance inventory: what an operator and an
//! integrator see.
//!
//! Split out of `conformance/probes.rs`, which had grown past the module line
//! cap.

use super::super::{AreaProbe, SourceProbe};

pub(super) static PROBES: &[AreaProbe] = &[
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
                ["isolate", "IsolatedWorkspace", "capture", "merge"]
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
            "rust/memory/worker.rs",
            ["claim", "heartbeat", "process_one", "complete", "retry"]
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
                "rust/rpc/acp/agent.rs",
                [
                    "build_agent",
                    "request_id",
                    "cancel_session",
                    "Drop for AcpState"
                ]
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
                "verified-update-manifest",
                "rust/update/manifest.rs",
                ["verify_envelope", "verify_artifact", "checksum mismatch"]
            ),
            source!(
                "transactional-update-rollback",
                "rust/update/transaction.rs",
                ["install", "recover", "previous binary restored"]
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
                "audit_ui_honesty_paths",
                "executable-without-health",
                "behavior_complete"
            ]
        )],
    },
];
