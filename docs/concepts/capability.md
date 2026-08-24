# Capability

How does Jeden know, at any moment, exactly which tools, slash commands,
views, extensions, and services exist in this checkout — and which of them
are actually executable? The capability registry (`rust/capability/mod.rs`):
a versioned, atomically-published snapshot of typed descriptors, rebuilt on
demand and inspectable with `jeden capabilities`.

## The descriptor

`CapabilityDescriptorV2` — every surface is one of these:

| Field | Meaning |
|---|---|
| `id` | ≤256 bytes of `[A-Za-z0-9/:-_.]`, e.g. `tool/read_file`, `slash/setup`, `service/capability-registry` |
| `kind` | `tool`, `slash-command`, `view`, `extension`, `plugin-contribution`, `mcp`, `skill`, `agent`, `rule`, `service` |
| `source` | the provider that contributed it |
| `version` | the Jeden version |
| `operations` | up to 64 operation names |
| `provenance` | `{provider, artifact_digest}` — builtins carry `builtin:<crate>:<version>` |
| `dependencies` | up to 64 capability ids |
| `health` | `{state, detail?}` — `healthy`, `degraded`, `unavailable`, `disabled`; only the first two are executable |
| `policy` | `read-only`, `approval-required`, `sandboxed`, `host-managed` |
| `ui` | `{label, description, visible, executable, action?}` |
| `target` | typed dispatch: `builtin-tool`, `extension-tool`, `mcp-tool`, `builtin-slash`, `file-slash`, `native-view`, `extension`, `declarative`, `mcp-server`, `service`, `none` |
| binding (flattened) | `input_schema_id`, `output_schema_id`, `handler_id`, `requested_grants`, `effective_grants` |
| `generation` | the snapshot generation that published it |
| `health_checked_at`, `health_evidence_id` | when and by what evidence health was asserted |

**Coherence rule:** a descriptor may claim `executable` only when its
binding is coherent — non-empty schema ids and handler id, and
`effective_grants ⊆ requested_grants` — and its health is executable.
Normalization silently strips the executable claim otherwise; a candidate
that still claims it is rejected with the diagnostic
`capability '<id>' declares an executable surface without coherent handler,
schemas, grants, and health; descriptor rejected`.

## The snapshot

`CapabilitySnapshot` is published atomically (arc-swap) and rebuilt when
invalidated or when the cwd changes:

- `registry_version` — 2.
- `generation` — monotonic per rebuild.
- `descriptors` — bounded to 4,096 (`MAX_CAPABILITIES`); overflow appends
  the diagnostic `capability registry reached bounded limit of 4096
  descriptors`.
- `diagnostics` — every rejected or conflicting candidate, kept beside the
  winners.

Providers contribute in a fixed order — builtin tools, runtime ops, tool
services, builtin slash commands, native views, editor/attachment/keymap
surfaces, roadmap, file-based slash commands, extensions, MCP — and
first-wins on id: a duplicate is recorded as
`duplicate capability id '<id>': first source '<a>' wins over '<b>'`.
An invalid id is `invalid capability id '<id>' from <source>; descriptor
rejected`. A failing extension runtime degrades to a `service/extensions`
descriptor whose health carries the error instead of hiding it.

## Observing it

```sh
jeden capabilities [--json] [--cwd path]
```

Captured from a fresh checkout:

```
Capability registry v2 generation 1: 212 descriptors, 0 diagnostics
- Tool: 66/72 available
- SlashCommand: 72/76 available
- View: 58/59 available
- Service: 3/5 available
```

`--json` prints the entire snapshot. `jeden doctor` probes several
capability-backed subsystems (`extensions`, `lsp`, `browser`, `collab`) and
reports `no configured capability was discovered` for empty ones —
degraded, not failed. Each session records a `capability_generation`
[ledger event](event-envelope.md), pinning which registry generation the
turn saw.

## Not to be confused with

- **RPC capabilities** — the fixed feature banner `jeden rpc` prints
  (`{"protocolVersion":1,"prompt":true,...}`); see [rpc](../rpc.md). That
  is a protocol handshake, not this registry.
- **Grants** — `requested_grants`/`effective_grants` name permissions a
  capability wants and holds; approval of individual tool calls is the
  approval gate in [what-is-jeden](../what-is-jeden.md#jailed-approval-gated-tools).
- **The frozen released surface** — `released-surface.json` pins the
  command vocabulary a release ships; the registry describes what this
  build in this checkout can do right now.
