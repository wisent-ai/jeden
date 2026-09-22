//! The first half of the conformance inventory: how a turn actually runs.
//!
//! Split out of `conformance/probes.rs`, which had grown past the module line
//! cap.

use super::super::{AreaProbe, SourceProbe};

pub(super) static PROBES: &[AreaProbe] = &[
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
        sources: &[
            source!(
                "durable-checkpoint-rewind",
                "rust/agent/runtime/recorder.rs",
                ["record_checkpoint", "fn rewind", "pending_tool_results"]
            ),
            source!(
                "active-session-lineage",
                "rust/session/store.rs",
                ["active_lineage", "append_with_parent", "Rewind"]
            ),
        ],
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
        sources: &[
            source!(
                "ranged-file-read",
                "rust/tool_runtime/read/files.rs",
                ["ranged_text", "read_file", "read_binary_file"]
            ),
            source!(
                "recursive-search",
                "rust/tool_runtime/exec/search.rs",
                ["discover", "search_files", "grep_regex"]
            ),
            source!(
                "sqlite-read",
                "rust/tool_runtime/read/sqlite.rs",
                ["read_sqlite", "SQLITE_OPEN_READ_ONLY"]
            ),
        ],
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
        sources: &[
            source!(
                "browser-service",
                "rust/tool_services/browser.rs",
                ["BrowserService", "execute", "health"]
            ),
            source!(
                "debugger-service",
                "rust/tool_services/debugger.rs",
                ["DebuggerService", "dap_request", "wait_response"]
            ),
            source!(
                "github-service",
                "rust/tool_services/github.rs",
                ["GithubService", "execute", "health"]
            ),
            source!(
                "ssh-service",
                "rust/tool_services/ssh.rs",
                ["SshService", "execute", "health"]
            ),
            source!(
                "web-service",
                "rust/tool_services/web.rs",
                ["WebService", "execute", "health"]
            ),
        ],
    },
    AreaProbe {
        area: "image-inspect-generate-i-tts",
        sources: &[source!(
            "media-service",
            "rust/tool_services/media.rs",
            ["MediaService", "post_json_fallback", "image_metadata"]
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
                ["checkpoint", "rewind", "list_checkpoints"]
            ),
            source!(
                "lineage-safe-pending-actions",
                "rust/cli/sessions.rs",
                ["append_rewind_entry", "active_entries", "pending_terminal"]
            ),
        ],
    },
];
