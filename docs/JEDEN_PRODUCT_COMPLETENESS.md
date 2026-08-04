# Jeden Product Completeness Contract

## Purpose

This is the canonical record of every audited capability gap, the automated target architecture, implementation order, acceptance gates, and native Jeden product identity. The external reference harness was used only as an evidence source for mature behavior classes. Jeden must not copy its brand, visual outline, terminology, glyphs, command organization, or UI composition.

## Non-negotiable rules

1. A command, picker, accepted ID, installed record, or status line is not a capability unless a live backend executes the behavior.
2. Provider, model, tool, command, plugin, MCP, skill, rule, agent, and UI inventories are generated from one capability registry, never duplicate hard-coded lists.
3. No delivered stubs, no-ops, fake fallbacks, compatibility aliases, deferred markers presented as reloads, or ownership messages presented as mutations.
4. Every state change is durable, cancellable, recoverable, and represented in a typed session graph.
5. Discovery, migration, activation, reconciliation, health checking, and conformance are automatic.
6. The native TUI preserves terminal scrollback, is Unicode-correct, and has clean deterministic non-TTY protocols.

## Status vocabulary

- `complete`: full contract implemented and behaviorally verified.
- `partial`: real subset exists but required semantics or guarantees are absent.
- `surface-only`: UI/config/listing exists without the corresponding runtime.
- `missing`: no executable implementation.
- `external-uncontracted`: an external Wisent service may own it, but Jeden lacks a typed testable API.

## Quantitative baseline

- Auth: provider picker generated from the Weles registry; typed device-code/paste/API-key login and tracked logout execute locally.
- Models: one OpenAI Chat Completions-shaped router protocol; automatic Brama catalog discovery with retry and cache.
- Tools: fifty-three registered low-level operations, not fifty-three complete session-scoped tool families.
- Settings: sixteen native schema keys.
- Extensions: factories imported in a sandboxed host, ABI-checked, isolated, with generation teardown.
- MCP: stdio plus streamable HTTP; persistent per-session manager with cache, health, and reconnect.
- Sessions: versioned typed events with entry graph, active leaf, checksums, and migrations.

# Complete gap matrix

## Authentication, providers, models, and routing

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| Provider discovery | Weles registry plus extension entries generate the picker | Versioned Weles provider registry generating auth UI | complete | P0 |
| Login | Typed device-code/paste/API-key operations with completion tracking; no generic browser OAuth | Execute typed OAuth/device/API-key operations and track completion | partial/strong | P0 |
| Logout | Tracked Weles logout operation; state refresh implicit | Revoke/remove account and refresh state | partial/strong | P0 |
| Account lifecycle | Typed Weles contract: status, expiry, refresh, auto-refresh, logout; no rotation/disable | Weles account references, status, expiry, refresh, rotation, disable | partial | P0 |
| Credential precedence | Ad-hoc env bearer/secret plus Weles account pool; no precedence policy | Explicit runtime/config/account/environment policy | missing | P1 |
| Model catalog | Versioned Brama catalog with availability, limits, modalities, tools, reasoning, cost, standby routes | Brama catalog: provider, availability, limits, modalities, tools, reasoning, cost, standbys | complete | P0 |
| Model discovery | Automatic catalog fetch with retry/cache plus extension-contributed entries | Automatic remote/local discovery owned by Brama registry | partial/strong | P1 |
| Local/custom providers | Extension-registered descriptors plus router URL override; no explicit keyless state | Registered descriptors and explicit keyless/auth state | partial | P1 |
| Wire protocol | OpenAI chat shape | Versioned normalized Wisent event stream | partial | P0 |
| Stream lifecycle | Text delta plus final tools | Typed text/thinking/tool/usage/retry/route/end/error events | partial | P0 |
| Stream corruption | Typed terminal error on malformed chunks and early EOF; loss always reported | Typed terminal error; loss always reported | complete | P0 |
| Tool argument streaming | Strict final parse | Incremental validated deltas and authoritative final parse | partial | P1 |
| Model metadata | Catalog limits, modalities, reasoning, tools, cache prices, promotion | Context/output limits, modalities, reasoning, tools, cache, promotion | complete | P0 |
| Availability validation | Catalog resolve enforced at config build and /model; virtual routes bypass | Resolve to live permitted catalog entry before request | partial/strong | P0 |
| Provider/model policy | Unwired policy engine; nothing enforced before routing | Project/path-scoped policy enforced before routing | missing | P1 |
| Canonical families | Catalog standby/promotion equivalence edges drive failover; no family abstraction | Stable family/equivalence metadata | partial | P2 |
| Automatic retry | Classified retry with jitter, retry-after, cancellation, recorded events | Classified retry, jitter, retry-after, idempotency, cancellation | partial/strong | P0 |
| Failover | Explicit config/catalog standby policy with recorded route-change events | Explicit standby policy and route-change events | partial/strong | P0 |
| Context promotion | Metadata-driven route promotion on context overflow precedes compaction | Metadata-driven promotion before compaction | complete | P1 |
| Compatibility shaping | Fixed body | Brama-owned adapters declared through capabilities | external-uncontracted | P1 |
| Usage/cost | Real with catalog prices, quota views, billing attribution; cost omitted when price missing | Catalog prices, quotas, attribution, missing-data indicators | partial | P2 |
| Secret protection | Automatic redact/obfuscate on model-bound copies; no typed provenance | Automatic redaction/obfuscation with provenance | partial | P0 |

## Tool runtime

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| Text selectors | Line ranges, multi-ranges, raw/conflict modes, bounded streaming | Real ranges, multi-ranges, raw/conflict modes, bounded streaming | complete | P0 |
| Directory depth | Recursive deterministic sorted listings with type/size metadata | Recursive deterministic metadata-rich listings | partial/strong | P1 |
| Binary/images | Base64 hard cap | Typed complete blocks, resize, artifacts | partial | P1 |
| Documents/notebooks | Text extraction plus notebook cell round-trip on write | Rich formats and editable notebook cell round-trip | partial | P2 |
| Archive/SQLite writes | SHA-guarded entry upsert/delete and row insert/update/delete | Safe entry/row create, update, delete | complete | P2 |
| Internal URIs | read/write route artifact/mcp/http plus archive/sqlite; agents/memory/skills/ssh/issues/PR absent | Unified artifacts/agents/memory/skills/MCP/SSH/issues/PR router | partial | P1 |
| File write | SHA guarded | Diagnostics, generated policy, executable handling, invalidation | partial | P1 |
| Anchored edit | Real | Snapshot recovery, parser blocks, diagnostics, no-op guard | partial | P1 |
| Grep/glob | Parallel gitignore-aware deterministic scans; cancel aborts without partial results | Parallel cancellable deterministic scans with partial results | partial | P1 |
| AST search/edit | Bounded tree-sitter search and preview/apply/discard rewrite | Structural search and preview/apply rewrite | complete | P1 |
| LSP | Persistent bounded client: diagnostics, navigation, rename, actions, formatting, lifecycle | Diagnostics, navigation, rename, actions, formatting, lifecycle | complete | P1 |
| Shell | Blocking sh plus persistent PTY session with resize, streaming, cancellation, artifacts | Session shell, PTY, stream, cancellation, hardening, artifacts | partial | P0 |
| Process | Process tree with group signals, descendant cleanup, bounded streamed capture | Process groups, descendant cleanup, signals, stream, recovery | partial | P0 |
| Background jobs | Durable scheduler: spawn, poll, cancel, deliver, merge, health, orphan reaping | Durable manager: poll, cancel, delivery, revival, cleanup | partial/strong | P0 |
| Python/JS eval | Persistent cancellable kernels with rich display artifacts and reset | Persistent cancellable kernels with rich displays and reset | complete | P1 |
| Browser | Chromium/CDP tabs, actions, screenshots via embedded bridge | Chromium/CDP tabs, actions, screenshots, cancellation | partial | P1 |
| Debugger | DAP session lifecycle plus typed request passthrough | DAP lifecycle and inspection | partial | P1 |
| Web search | Provider-routed search with citations and provider standby; key required | Search routing, citations, sources, provider standby | partial | P1 |
| GitHub | gh-backed issues/PRs/search/Actions plus worktrees and guarded push | Issues, PRs, search, Actions, worktrees, guarded push | complete | P2 |
| SSH | Reusable multiplexed connections with remote read/search/write/exec | Connection manager and remote URI read/search/write/exec | partial | P1 |
| Image inspect/generate | Metadata inspect; router-backed generate/edit to artifacts; no vision analysis | Vision analysis and provider-routed generation/edit | partial | P2 |
| TTS | Provider-routed synthesis to artifact via media router | Provider-routed synthesis to artifact | complete | P3 |
| Pending actions | Persistent TTL preview/apply/discard registry backing AST rewrite | Persistent preview/apply/discard registry | partial | P0 |
| Checkpoint/rewind | Ledger checkpoints with list and branch-safe rewind to a new leaf | Graph checkpoint and branch-safe rewind | complete | P1 |
| Output spill | Shared bounded sink auto-spills full output to artifact with sha256 | Shared bounded sink preserving full output automatically | complete | P0 |
| Cancellation | One shared token propagated to children and polled inside tool operations | One token through all active operations | partial/strong | P0 |
| Concurrency | Sequential | Declared shared/exclusive semantics and bounded parallelism | missing | P1 |

## Sessions, context, and persistence

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| Session schema | Versioned typed events with ID, parent, leaf, checksums, migrations | Versioned typed entries with ID, parent, leaf, migrations | complete | P0 |
| Durability | Fsync'd append, atomic rename plus dir-sync state writes, explicit tail recovery | Atomic/latched persistence and explicit recovery | complete | P0 |
| Corruption | Checksum-verified reads; malformed lines hard-error; truncated tail detected, preserved, reported | Detect, preserve, recover, report | complete | P0 |
| Resume | Ledger replay of snapshots, checkpoints, tool results, compaction cut points; no modes/todos/MCP | Faithful model/messages/tools/modes/todos/compaction/MCP reconstruction | partial | P0 |
| Session switch | Incomplete | Transactional switch with rollback | partial | P0 |
| Tree | Flat branch list plus session names, branch/checkpoint labels, and search-sessions; no entry-graph navigation or summaries | Entry graph navigation, search, labels, summaries | partial | P0 |
| Branch/fork | Durable lineage and full-window snapshot seed across restart; artifacts not carried | Full semantic lineage and artifact preservation across restart | partial | P0 |
| Compaction | Durable typed cut point with auto-trigger; no recent-suffix retention or strategies | Typed durable cut point, recent suffix, reload, strategies | partial | P0 |
| Handoff | Durable brief artifact and durable parent-linked target session | Durable parent-linked target session | complete | P0 |
| Export/share | Raw events/local encrypted file | Rich typed export and redacted compressed E2EE share | partial | P2 |
| Artifact lifecycle | Session files | Content-addressed blobs, graph refs, fork/export, GC | partial | P1 |
| Context files | Walk-up loader with imports, precedence, provenance, budgets; no watch | Walk-up discovery, imports, precedence, provenance, watch | partial | P0 |
| Sticky rules | Always-applied registry with precedence and provenance injected into system prompt | Always-applied registry and provenance | complete | P0 |
| Rule matching/TTSR | Prompt-time rule matching wired; no typed stream interventions | Runtime matching and typed stream interventions | partial | P1 |

## Memory and collaboration

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| Manual memory | SQLite WAL/FTS5 store with stable IDs, revisions, tombstones, scope locks | Locking, stable IDs, typed backend | complete | P1 |
| Recall | FTS5 with temporal decay, confidence, typed provenance; semantic hybrid behind an unwired provider | Semantic/fact/temporal scores and provenance | partial/strong | P2 |
| Extraction | None | Background extraction from closed sessions | missing | P2 |
| Consolidation | Model compaction summary persisted as consolidated memory; leased Consolidator unwired | Leased model consolidation with redaction | partial | P2 |
| Queue | Durable SQLite jobs with lease, heartbeat, backoff retry, status; manual drain | Durable worker, wake, lease, heartbeat, retry, status | partial/strong | P2 |
| Generated skills | None | Managed verified playbooks | missing | P3 |
| Pre-compaction memory | Bounded recall-ranked context from the store injected before compaction | Bounded relevant context from backend | complete | P2 |
| E2EE | Sealed versioned frames with role checks wired into /collab, /join, relay | Integrate into full collaboration protocol | partial/strong | P1 |
| Relay | Durable SQLite-WAL content-blind relay with /health and backpressure | Durable content-blind relay with health | complete | P2 |
| Live replication | Lifecycle markers | Transcript, stream, state, tools, participants, agents | missing | P2 |
| Guest control | Role-permissioned frames enforced in protocol and relay; no host prompt/abort consumer | Permissioned prompt, abort, view/full roles | partial | P2 |
| Write authorization | Separate role-bound write tokens, hashed server-side, rotatable/revocable | Separate revocable write capability | partial/strong | P2 |
| Web guest | None | Native client generated from protocol | missing | P3 |

## MCP, extensions, plugins, skills, and agents

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| MCP config | Two JSON roots | Validated multi-source descriptors, substitutions, auth, provenance | partial | P1 |
| MCP transports | Stdio plus streamable HTTP with session id and SSE parsing; no OAuth/SSE-listen/server requests | Standard stdio JSONL, HTTP, SSE, OAuth, server requests | partial | P1 |
| MCP lifecycle | Persistent per-session manager: parallel connect, cache, health, reconnect, teardown | Persistent parallel manager, cache, health, reconnect, teardown | partial/strong | P0 |
| Dynamic tools | Live tools/list registered as native executable tools; list-changed refresh | Live tools/list registration and refresh | partial/strong | P1 |
| Resources/prompts | List/read/get with per-server cache and list-change refresh; no templates/subscriptions | Templates, subscriptions, notifications, live refresh | partial | P2 |
| Reconnect | Exponential backoff, circuit breaker, state-resetting reconnect with re-list | Real state recovery with backoff/circuit breaker | partial/strong | P1 |
| Extension discovery | Sandboxed Node host import, ABI-checked activation, isolated grants, generational teardown | Import factories, ABI, initialize, isolate, teardown | partial/strong | P0 |
| Event bus | Closed typed session journal plus durable outbox and narrow SDK live stream | Typed session/provider/turn/message/tool/approval/retry events | partial | P1 |
| Tools/commands/UI | Single versioned registry generating tool/command/view inventories with health and conflicts | Unified live registration and native renderers | partial/strong | P1 |
| Live reload | Atomic in-process rebuild with generation; failed builds keep the previous registry | Atomic registry rebuild and rollback | partial/strong | P1 |
| Shell hooks | Five events | Adapter onto full typed event contract | partial | P1 |
| Custom tools | Fresh Node per call behind versioned ABI with schema, cancel, grants, artifacts | Versioned ABI, validation, cancel, updates, context, artifacts | partial/strong | P1 |
| Marketplace | Real management | Preserve and expose activation health | partial/strong | P1 |
| Plugin activation | Tools, extensions, skills, agents, rules, commands, hooks, providers, models; no MCP/LSP | Activate tools/extensions/skills/agents/rules/MCP/LSP | partial | P1 |
| Package/link plugins | Signed install/upgrade/remove with lock, feature resolver, transactional rollback, dev-link | Versioned lifecycle, lock, features, rollback, doctor | partial/strong | P2 |
| Skills/rules | Multi-source SKILL.md/rules with precedence, safe assets, matcher-based prompt injection | Multi-source declarative discovery and safe internal URIs | partial/strong | P1 |
| Custom agents | Declarative agents with tools/model/output/spawn/skills, discovered multi-source, enforced at spawn | Tools/model/output/spawn/skills definitions | partial/strong | P1 |
| Task scheduler | Batch DAG with slot-bounded concurrency, isolated sandboxes, poll/health, schema-validated output | Batch DAG, bounded concurrency, isolation, progress, typed output | complete | P1 |
| Job manager | Durable atomic job records with pid-liveness recovery; leased fencing coordinator unwired | Durable ownership and recovery | partial/strong | P0 |
| Agent communication | Durable mailboxes with send/inbox/wait/wake via irc tool | Mailboxes, direct messages, wait, wake | complete | P2 |
| Isolation | Automatic apfs-clone/git-worktree/copy isolation with patch capture and merge | Automatic platform copy-on-write/worktree and merge/capture | complete | P1 |

## Native terminal interface

Jeden must use an original Wisent/Jeden design system. It must not reuse another product's splash structure, status outline, glyph system, color associations, terminology, or command organization.

| Gap | Current | Required native end state | Status | Priority |
|---|---|---|---|---|
| Identity | Mixed legacy labels/fixed palette | One explicit Jeden terminology and visual token system | partial | P1 |
| Editor | Cursor, selection, word/line movement, undo/redo, history, multiline, external editor | Cursor, selection, movement, deletion, undo, history, multiline, external editor | complete | P0 |
| Paste | Bracketed paste, escape/control stripping, clipboard text+image, bounded large-paste cap | Bracketed paste, fragmented escapes, clipboard, large-paste safety | partial/strong | P1 |
| Attachments | File/clipboard-image add, list, remove, size/type limits, image-capability failover | File/image add, preview, remove, capability validation | partial/strong | P1 |
| Streaming | Text live block | Structured text/thinking/tool/progress components | partial | P1 |
| Steering/follow-up | Queued follow-up/steer during streaming with recall edit; steer has no backend provider | Editable queued delivery during streaming | partial | P1 |
| Keybindings | Action IDs and conflict diagnostics; bindings fixed in code | Action IDs, configurable bindings, conflict diagnostics | partial | P2 |
| Picker | Searchable panel | Keep; add focus, layers, argument completion, lifecycle | partial/strong | P2 |
| Confirmation | Default cancel | Add risk, origin, scope, consequences | partial/strong | P2 |
| Approvals | Binary key | Rich policy-aware decisions with scoped persistence | partial | P1 |
| Renderer | Sticky live region | Audited scrollback ledger, resize recovery, synchronized writes | partial | P1 |
| Unicode | Character count | ANSI-safe grapheme and terminal-width model | partial | P0 |
| Terminal protocols | Basic events | Capability probing, normalized keys, suspend/resume, cleanup | partial | P1 |
| Images | None | Capability-detected inline rendering with text fallback | missing | P2 |
| Themes/accessibility | Dark+light presets, custom theme.json, NO_COLOR, mono/high-contrast, emphasis signals | Native dark/light themes, custom schema, non-color signals | partial/strong | P2 |
| Completion | Live registry-driven slash completion with selection; no path completion | Live capabilities, args, descriptions, health | partial/strong | P1 |
| Status | Live tokens/cost, quota, route health, jobs, services, modes, context; no collab/errors | Live context, cost, route, jobs, collab, errors | partial/strong | P1 |
| Non-TTY | Mixed terminal framing | Clean line and JSON protocols without ANSI | partial | P0 |
| RPC/ACP UI | Structured elicitation and permission bridge on both RPC and ACP | Structured elicitation and approval bridge | complete | P1 |

## Reliability and platform

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| Cancellation | Token through HTTP stream, tools, process trees, extensions, ACP agents; MCP fixed-timeout only | Token through HTTP/tools/process trees/MCP/extensions/agents | partial | P0 |
| Process ownership | Direct-child kill | Groups, descendant cleanup, grace/force, recovery | partial | P0 |
| Deadlines | Hierarchical effective deadline plus first-event/idle stream timeouts | Hierarchical deadlines and first-event/idle timeout | partial/strong | P1 |
| Retry | Classified retry with jitter, Retry-After, cancellation, failover, recorded events | Classified retry/failover with cancellation | partial/strong | P0 |
| Output bounds | Shared BoundedOutput sink with artifact spill and sha256 across process/pty/kernel/tools/extensions | Shared sink and artifact provenance | partial/strong | P0 |
| Doctor | Active probes for brama/weles/storage/process/MCP/extensions/LSP/browser/collab/task/memory/keymap | Active probes for all services and runtimes | complete | P1 |
| Updater | Signed staged release with journaled atomic swap, rollback recovery, SBOM+provenance verification, post-install health probe | Verified staged release, atomic swap, rollback, health | complete | P1 |
| SDK | Public lib exports sdk::AgentSession and async SessionClient with events, abort, resume, dispose | Public AgentSession library and events | complete | P0 for embedding |
| RPC | Correlated long-lived jeden-rpc stdio server plus mTLS headless daemon with events, abort, dispose, idempotency, replay | Correlated long-lived protocol, events, abort, dispose | complete | P0 for automation |
| ACP | ACP v1 stdio adapter over sdk::AgentSession with load, prompt streaming, cancel, close | Adapter over the same public session API | complete | P1 |
| Platforms | Shell assumptions | Explicit macOS/Linux/Windows contracts | partial | P2 |
| Scan cache | None | Policy-keyed TTL/invalidation/recheck cache | missing | P2 |
| Conformance | Area-wide suite of contract, inventory, and digest-bound behavior-evidence probes plus generated capability/UI-honesty audit | Generated capability and behavioral contract suite | partial/strong | P0 |
| UI honesty | Surfaces exceed backend | Build/runtime gate forbids active UI for inactive capability | partial | P0 |

# Existing partial paths requiring clean cutover

Login, logout, model/provider selection, extensions, reload plugins, browser, debugger, SSH, jobs, MCP reconnect, MCP notifications, memory enqueue/rebuild, tree, fork, resume, collaboration, share, and advisor must either gain the complete backend above or be removed. No compatibility no-op remains in the final product.

# Automated target architecture

```text
Jeden native TUI / line protocol / JSON / RPC / ACP
                         |
                 public AgentSession API
                         |
          typed session graph + capability registry
                         |
 operation context + process manager + output/artifact sinks
             |                 |                 |
        tool runtime      MCP/extensions      task/job runtime
             +-----------------+-----------------+
                               |
                    Brama model control plane
                    Weles identity control plane
```

## Capability registry

Every feature publishes a versioned descriptor with source, operations, dependencies, health, and UI affordances. TUI, CLI, SDK, RPC, doctor, and conformance consume exactly this registry.

## Session graph

Every message, thinking block, tool call/result, approval, route change, retry, compaction, handoff, branch, todo, artifact, subagent event, collaboration event, and extension entry is typed and versioned. Resume, branch, fork, tree, rewind, export, share, and collaboration are transactions or views over the graph.

## Operation context

Every active operation receives cancellation, deadline, progress, output, artifacts, approvals, and session handles. No process, request, or output buffer is untracked.

## Fully automated lifecycle

- Startup discovers, validates, deduplicates, activates, health-checks, and reports every capability source.
- Startup migrates schemas, detects damaged tails, preserves damage, recovers valid state, rebuilds indexes, and runs invariants.
- Plugin install stages every capability, activates atomically, tears down the prior version, and rolls back on failure.
- MCP discovery registers live tool schemas without static duplication.
- Context and rules are discovered and watched after every cwd change.
- Memory workers use durable leases, heartbeats, retries, and observable status.
- Subagents use a bounded DAG scheduler, isolation, cancellation propagation, durable child sessions, and automatic cleanup.

# Native Jeden identity

The native product mark is `jeden.`: lower-case, calm, and completed by one period that represents a verified operation. The design language communicates controlled infrastructure, explicit state, calm feedback, and trustworthy transitions. Transcript geometry is flat and left-aligned; a bounded editor dock owns temporary interaction; panels are transient sheets rather than boxed message cards. Components use semantic color tokens, spacing cells `0/1/2/4`, and redundant text labels so color or glyph is never the sole state signal. No animal mascot, large ASCII logo, borrowed command grouping, foreign status-line outline, or copied glyph/color association is permitted. Normal scrollback remains authoritative; live components stay bounded; Unicode and resize are lossless; non-TTY output is clean.

# Dependency order

1. Capability registry and honest generated surfaces.
2. Typed session graph, migrations, faithful replay, branch/fork/tree.
3. Operation context, process manager, output sink, blob artifacts.
4. Brama/Weles contracts, normalized stream, retry, secret policy.
5. Context/rules and durable compaction/handoff/rewind.
6. Public SDK and RPC substrate.
7. Persistent MCP, extensions, plugin activation, task/job managers.
8. Advanced tools and autonomous memory.
9. Full collaboration.
10. Native editor, renderer, attachments, themes, accessibility.
11. Continuous conformance and removal of every surface-only path.

# Automated acceptance gates

A capability is complete only when all applicable gates pass:

1. Live healthy descriptor is registered.
2. UI is generated from that descriptor.
3. CLI, TUI, SDK, RPC, and resumed-session paths share one handler.
4. Cancellation interrupts the operation and descendants.
5. Full output is preserved through the artifact sink.
6. Restart reconstructs identical semantic state.
7. Failure is typed and never silently reported as success.
8. Concurrency and activation are bounded and deterministic.
9. Migration is automatic.
10. Automated scenarios cover success, failure, cancellation, and restart.

The conformance runner must automatically cover auth flow fixtures, catalog refresh, malformed streams, provider retry/failover, session restart after tool/compaction/fork, process-tree cancellation, artifact spill, MCP reconnect and list changes, plugin activation/upgrade/rollback, subagent cancellation and isolation, Unicode/resize/paste, non-TTY cleanliness, and UI/backend honesty.

# Evidence boundary

This contract derives from current Jeden Rust source, dynamic Jeden inventories, installed reference documentation and registries, and six domain source audits. External Brama/Weles behavior is uncontracted unless represented by a versioned API consumed here. Completion is determined by automated conformance, not documentation claims or command names.
