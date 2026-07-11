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

- Auth: three inspect-only choices; zero locally executed login flows.
- Models: one OpenAI Chat Completions-shaped router protocol; no catalog discovery.
- Tools: 43 registered low-level operations, not 43 complete session-scoped tool families.
- Settings: seven native schema keys.
- Extensions: candidates are listed but factories are not loaded.
- MCP: stdio only; one process and handshake per operation.
- Sessions: untyped JSONL events without entry graph, active leaf, or migrations.

# Complete gap matrix

## Authentication, providers, models, and routing

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| Provider discovery | Three hard-coded choices | Versioned Weles provider registry generating auth UI | surface-only | P0 |
| Login | Displays credential plan | Execute typed OAuth/device/API-key operations and track completion | surface-only | P0 |
| Logout | Host-owned message | Revoke/remove account and refresh state | surface-only | P0 |
| Account lifecycle | No local contract | Weles account references, status, expiry, refresh, rotation, disable | external-uncontracted | P0 |
| Credential precedence | Router signing secret only | Explicit runtime/config/account/environment policy | missing | P1 |
| Model catalog | Route strings | Brama catalog: provider, availability, limits, modalities, tools, reasoning, cost, fallbacks | surface-only | P0 |
| Model discovery | None | Automatic remote/local discovery owned by Brama registry | missing | P1 |
| Local/custom providers | Router URL override | Registered descriptors and explicit keyless/auth state | partial | P1 |
| Wire protocol | OpenAI chat shape | Versioned normalized Wisent event stream | partial | P0 |
| Stream lifecycle | Text delta plus final tools | Typed text/thinking/tool/usage/retry/route/end/error events | partial | P0 |
| Stream corruption | Malformed chunks may be skipped | Typed terminal error; never silent loss | partial | P0 |
| Tool argument streaming | Strict final parse | Incremental validated deltas and authoritative final parse | partial | P1 |
| Model metadata | ID and optional prices | Context/output limits, modalities, reasoning, tools, cache, promotion | missing | P0 |
| Availability validation | Any route accepted | Resolve to live permitted catalog entry before request | missing | P0 |
| Provider/model policy | None | Project/path-scoped policy enforced before routing | missing | P1 |
| Canonical families | None | Stable family/equivalence metadata | missing | P2 |
| Automatic retry | Manual replay | Classified retry, jitter, retry-after, idempotency, cancellation | missing | P0 |
| Failover | Opaque router behavior | Explicit fallback policy and route-change events | external-uncontracted | P0 |
| Context promotion | Same-route compaction | Metadata-driven promotion before compaction | missing | P1 |
| Compatibility shaping | Fixed body | Brama-owned adapters declared through capabilities | external-uncontracted | P1 |
| Usage/cost | Real with configured prices | Catalog prices, quotas, attribution, missing-data indicators | partial | P2 |
| Secret protection | No outbound layer | Automatic redaction/obfuscation with provenance | missing | P0 |

## Tool runtime

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| Text selectors | Declared but whole-file read | Real ranges, multi-ranges, raw/conflict modes, bounded streaming | partial | P0 |
| Directory depth | Shallow | Recursive deterministic metadata-rich listings | partial | P1 |
| Binary/images | Base64 hard cap | Typed complete blocks, resize, artifacts | partial | P1 |
| Documents/notebooks | Basic extraction | Rich formats and editable notebook cell round-trip | partial | P2 |
| Archive/SQLite writes | Read-only | Safe entry/row create, update, delete | missing | P2 |
| Internal URIs | Separate ad hoc operations | Unified artifacts/agents/memory/skills/MCP/SSH/issues/PR router | missing | P1 |
| File write | SHA guarded | Diagnostics, generated policy, executable handling, invalidation | partial | P1 |
| Anchored edit | Real | Snapshot recovery, parser blocks, diagnostics, no-op guard | partial | P1 |
| Grep/glob | Sync traversal, incomplete ignore | Parallel cancellable deterministic scans with partial results | partial | P1 |
| AST search/edit | None | Structural search and preview/apply rewrite | missing | P1 |
| LSP | None | Diagnostics, navigation, rename, actions, formatting, lifecycle | missing | P1 |
| Shell | Blocking sh | Session shell, PTY, stream, cancellation, hardening, artifacts | partial | P0 |
| Process | Direct child only | Process groups, descendant cleanup, signals, stream, recovery | partial | P0 |
| Background jobs | Detached metadata | Durable manager: poll, cancel, delivery, revival, cleanup | surface-only | P0 |
| Python/JS eval | New process per call | Persistent cancellable kernels with rich displays and reset | partial | P1 |
| Browser | Config UI only | Chromium/CDP tabs, actions, screenshots, cancellation | surface-only | P1 |
| Debugger | Diagnostics UI only | DAP lifecycle and inspection | surface-only | P1 |
| Web search | Known-URL fetch only | Search routing, citations, sources, fallback | missing | P1 |
| GitHub | Local Git inspection | Issues, PRs, search, Actions, worktrees, guarded push | missing | P2 |
| SSH | Host config only | Connection manager and remote URI read/search/write/exec | surface-only | P1 |
| Image inspect/generate | Metadata only/none | Vision analysis and provider-routed generation/edit | missing | P2 |
| TTS | None | Provider-routed synthesis to artifact | missing | P3 |
| Pending actions | Bridge no-op | Persistent preview/apply/discard registry | surface-only | P0 |
| Checkpoint/rewind | None | Graph checkpoint and branch-safe rewind | missing | P1 |
| Output spill | Manual artifacts | Shared bounded sink preserving full output automatically | missing | P0 |
| Cancellation | Between-step flag | One token through all active operations | partial | P0 |
| Concurrency | Sequential | Declared shared/exclusive semantics and bounded parallelism | missing | P1 |

## Sessions, context, and persistence

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| Session schema | Untyped events | Versioned typed entries with ID, parent, leaf, migrations | partial | P0 |
| Durability | Append without latch | Atomic/latched persistence and explicit recovery | partial | P0 |
| Corruption | Invalid lines skipped | Detect, preserve, recover, report | partial | P0 |
| Resume | User/final text only | Faithful model/messages/tools/modes/todos/compaction/MCP reconstruction | partial | P0 |
| Session switch | Incomplete | Transactional switch with rollback | partial | P0 |
| Tree | Branch path list | Entry graph navigation, search, labels, summaries | surface-only | P0 |
| Branch/fork | Reduced or RAM-only history | Full semantic lineage and artifact preservation across restart | partial | P0 |
| Compaction | Whole-history RAM summary | Typed durable cut point, recent suffix, reload, strategies | partial | P0 |
| Handoff | Old artifact and RAM seed | Durable parent-linked target session | partial | P0 |
| Export/share | Raw events/local encrypted file | Rich typed export and redacted compressed E2EE share | partial | P2 |
| Artifact lifecycle | Session files | Content-addressed blobs, graph refs, fork/export, GC | partial | P1 |
| Context files | Documented, no loader found | Walk-up discovery, imports, precedence, provenance, watch | missing | P0 |
| Sticky rules | None | Always-applied registry and provenance | missing | P0 |
| Rule matching/TTSR | Inert complaint log | Runtime matching and typed stream interventions | surface-only | P1 |

## Memory and collaboration

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| Manual memory | JSONL notes | Locking, stable IDs, typed backend | partial | P1 |
| Recall | Lexical | Semantic/fact/temporal scores and provenance | partial | P2 |
| Extraction | None | Background extraction from closed sessions | missing | P2 |
| Consolidation | Bullet rebuild | Leased model consolidation with redaction | surface-only | P2 |
| Queue | File marker | Durable worker, wake, lease, heartbeat, retry, status | surface-only | P2 |
| Generated skills | None | Managed verified playbooks | missing | P3 |
| Pre-compaction memory | None | Bounded relevant context from backend | missing | P2 |
| E2EE | AES-GCM primitive | Integrate into full collaboration protocol | partial | P1 |
| Relay | File/in-memory log | Durable content-blind relay with health | partial | P2 |
| Live replication | Lifecycle markers | Transcript, stream, state, tools, participants, agents | missing | P2 |
| Guest control | None | Permissioned prompt, abort, view/full roles | missing | P2 |
| Write authorization | Shared key | Separate revocable write capability | missing | P2 |
| Web guest | None | Native client generated from protocol | missing | P3 |

## MCP, extensions, plugins, skills, and agents

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| MCP config | Two JSON roots | Validated multi-source descriptors, substitutions, auth, provenance | partial | P1 |
| MCP transports | Stdio only | Standard stdio JSONL, HTTP, SSE, OAuth, server requests | partial | P1 |
| MCP lifecycle | Process per request | Persistent parallel manager, cache, health, reconnect, teardown | missing | P0 |
| Dynamic tools | Generic calls/static native aliases | Live tools/list registration and refresh | partial | P1 |
| Resources/prompts | One-shot | Templates, subscriptions, notifications, live refresh | partial | P2 |
| Reconnect | Re-spawn probe | Real state recovery with backoff/circuit breaker | surface-only | P1 |
| Extension discovery | Candidate listing | Import factories, ABI, initialize, isolate, teardown | surface-only | P0 |
| Event bus | None | Typed session/provider/turn/message/tool/approval/retry events | missing | P1 |
| Tools/commands/UI | Separate limited paths | Unified live registration and native renderers | missing | P1 |
| Live reload | Marker for next run | Atomic registry rebuild and rollback | surface-only | P1 |
| Shell hooks | Five events | Adapter onto full typed event contract | partial | P1 |
| Custom tools | Fresh Node, limited shim | Versioned ABI, validation, cancel, updates, context, artifacts | partial | P1 |
| Marketplace | Real management | Preserve and expose activation health | partial/strong | P1 |
| Plugin activation | Mostly commands/hooks | Activate tools/extensions/skills/agents/rules/MCP/LSP | partial | P1 |
| Package/link plugins | None | Versioned lifecycle, lock, features, rollback, doctor | missing | P2 |
| Skills/rules | None | Multi-source declarative discovery and safe internal URIs | missing | P1 |
| Custom agents | None | Tools/model/output/spawn/skills definitions | missing | P1 |
| Task scheduler | One sync child | Batch DAG, bounded concurrency, isolation, progress, typed output | partial | P1 |
| Job manager | Detached metadata | Durable ownership and recovery | surface-only | P0 |
| Agent communication | None | Mailboxes, direct messages, wait, wake | missing | P2 |
| Isolation | Shared workspace | Automatic platform copy-on-write/worktree and merge/capture | missing | P1 |

## Native terminal interface

Jeden must use an original Wisent/Jeden design system. It must not reuse another product's splash structure, status outline, glyph system, color associations, terminology, or command organization.

| Gap | Current | Required native end state | Status | Priority |
|---|---|---|---|---|
| Identity | Mixed legacy labels/fixed palette | One explicit Jeden terminology and visual token system | partial | P1 |
| Editor | Append-only buffer | Cursor, selection, movement, deletion, undo, history, multiline, external editor | partial | P0 |
| Paste | None | Bracketed paste, fragmented escapes, clipboard, large-paste safety | missing | P1 |
| Attachments | None | File/image add, preview, remove, capability validation | missing | P1 |
| Streaming | Text live block | Structured text/thinking/tool/progress components | partial | P1 |
| Steering/follow-up | None | Editable queued delivery during streaming | missing | P1 |
| Keybindings | Hard-coded | Action IDs, configurable bindings, conflict diagnostics | partial | P2 |
| Picker | Searchable panel | Keep; add focus, layers, argument completion, lifecycle | partial/strong | P2 |
| Confirmation | Default cancel | Add risk, origin, scope, consequences | partial/strong | P2 |
| Approvals | Binary key | Rich policy-aware decisions with scoped persistence | partial | P1 |
| Renderer | Sticky live region | Audited scrollback ledger, resize recovery, synchronized writes | partial | P1 |
| Unicode | Character count | ANSI-safe grapheme and terminal-width model | partial | P0 |
| Terminal protocols | Basic events | Capability probing, normalized keys, suspend/resume, cleanup | partial | P1 |
| Images | None | Capability-detected inline rendering with text fallback | missing | P2 |
| Themes/accessibility | Fixed ANSI | Native dark/light themes, custom schema, non-color signals | missing | P2 |
| Completion | Static names | Live capabilities, args, descriptions, health | partial | P1 |
| Status | Placeholder fields | Live context, cost, route, jobs, collab, errors | surface-only | P1 |
| Non-TTY | Mixed terminal framing | Clean line and JSON protocols without ANSI | partial | P0 |
| RPC/ACP UI | None | Structured elicitation and approval bridge | missing | P1 |

## Reliability and platform

| Gap | Current | Required automated end state | Status | Priority |
|---|---|---|---|---|
| Cancellation | Boundary checks | Token through HTTP/tools/process trees/MCP/extensions/agents | partial | P0 |
| Process ownership | Direct-child kill | Groups, descendant cleanup, grace/force, recovery | partial | P0 |
| Deadlines | Fixed local values | Hierarchical deadlines and first-event/idle timeout | partial | P1 |
| Retry | None | Classified retry/failover with cancellation | missing | P0 |
| Output bounds | Inconsistent caps | Shared sink and artifact provenance | missing | P0 |
| Doctor | Config presence | Active probes for all services and runtimes | partial | P1 |
| Updater | Pull then build | Verified staged release, atomic swap, rollback, health | partial | P1 |
| SDK | Binary only | Public AgentSession library and events | missing | P0 for embedding |
| RPC | One-shot JSON only | Correlated long-lived protocol, events, abort, dispose | missing | P0 for automation |
| ACP | None | Adapter over the same public session API | missing | P1 |
| Platforms | Shell assumptions | Explicit macOS/Linux/Windows contracts | partial | P2 |
| Scan cache | None | Policy-keyed TTL/invalidation/recheck cache | missing | P2 |
| Conformance | Few focused tests | Generated capability and behavioral contract suite | missing | P0 |
| UI honesty | Surfaces exceed backend | Build/runtime gate forbids active UI for inactive capability | partial | P0 |

# Existing surface-only paths requiring clean cutover

Login, logout, model/provider selection, extensions, reload plugins, browser, debugger, SSH, jobs, MCP reconnect, MCP notifications, memory enqueue/rebuild, tree, fork, resume, collaboration, share, advisor, and placeholder status fields must either gain the complete backend above or be removed. No compatibility no-op remains in the final product.

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
