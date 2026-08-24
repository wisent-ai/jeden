# Sessions and ledgers

Where does Jeden's durable state live, and what shape does it have? Three
ledgers carry everything: the per-session event ledger under
`~/.jeden/sessions/`, the per-project usage ledger
`<cwd>/.jeden/usage.json`, and the per-project mode state
`<cwd>/.jeden/mode-state.json`. All of them are plain files on the
operator's disk; Jeden uploads none of them and never expires or deletes a
session transcript.

## The session directory

Each session is a directory `~/.jeden/sessions/<unix-seconds>-<6-char
suffix>/` (`JEDEN_SESSION_ROOT` overrides the root). It holds:

- `state.json` — session metadata: `version` (the ledger schema version,
  currently 2), `id`, `cwd`, `startedAt`, `activeLeaf` (the event id of the
  current lineage tip, mirrored atomically after every append), and
  `lineage` (`parentSession`, `parentEntry`) when the session was resumed
  from another.
- `transcript.jsonl` — the append-only event ledger.
- `artifacts/` — oversized tool results persisted as session artifacts and
  replaced in the model loop with a compact reference.

## The transcript ledger

`transcript.jsonl` holds one JSON event per line. Every event carries:

| Field | Meaning |
|---|---|
| `eventId` | `event-<timestamp>-<12-char suffix>`, unique within the ledger |
| `sessionId` | must match the session's `state.json` id |
| `parentId` | the previous active leaf (`null` only for the first event) |
| `sequence` | 1-based position; must increase by exactly 1 |
| `timestamp` | unix seconds as a string |
| `causationId` | must equal `parentId` |
| `correlationId` | inherited from the previous event; first event uses its own id |
| `schemaVersion` | 2 |
| `payload` | `{"type": ..., "data": ...}` from a closed vocabulary |
| `outbox` | transactional delivery seeds for the `memory`, `collaboration`, `telemetry`, and `remote_replication` consumers |
| `checksum` | SHA-256 over the event serialized with an empty checksum field |

The payload vocabulary is closed: a variant is added to the
`SessionPayloadV2` enum before a producer can persist it, so misspelled
event kinds cannot enter replay. It covers conversation events (`user`,
`assistant`, `assistant_raw`, `final`), tool events (`action`, `tool_call`,
`tool_result`, `approval`, `artifact`), context events (`context_snapshot`,
`compaction`, `auto_compaction`, `tool_prune`, `handoff`), lineage events
(`lineage`, `branch`, `checkpoint`, `rewind`), model-routing events
(`model_attempt`, `model_route`, `model_route_result`, `model_retry`,
`model_usage`, `usage_error`), plus goal-lifecycle, memory, roadmap, worker,
collaboration, interaction, telemetry-reference, terminal-outcome, and
pending-mutation events.

### Append and validation

Every append re-reads the ledger, seals the new event's checksum, and
validates it against the prior events before writing: the sequence must be
exactly `len + 1`, the event id must be fresh, the parent must be the
current active leaf, causation must equal parent, and the outbox seeds must
be exactly the pending set for all four consumers. The line is then written
with `sync_data`, and `state.json`'s `activeLeaf` is updated through a
temporary file, `fsync`, rename, and directory sync.

Reads verify every event's schema version and checksum and replay the same
validation. A transcript whose final line is torn (no trailing newline,
malformed JSON) is read up to the last valid event and marked as having a
recovered truncated tail; such a ledger refuses further appends with
`cannot append ...: transcript has a recovered truncated tail; resume into a
child session`. Legacy (pre-v2) lines are migrated in memory and the file is
rewritten as v2 on the next append.

### Lineage, checkpoints, rewind

The ledger is a tree, not just a list: `/checkpoint [label]` records the
exact model-visible context, and `/rewind <checkpoint-event-id>` appends a
`rewind` event whose parent is the checkpoint — starting a new active
lineage without deleting abandoned history. A `rewind` is only accepted when
its source is the current active leaf and the named checkpoint is a real
`checkpoint` event on the active lineage. Resuming a session (`jeden resume
<session> "continue"`) starts a fresh session whose `state.json` records the
parent session and entry.

### Inspecting sessions

```sh
jeden sessions [limit]
jeden show <session-id-or-path>
jeden export <session-id-or-path> [output.json]
jeden artifacts <session-id-or-path>
jeden artifact <session-id-or-path> <name> [output]
jeden resume <session> "continue"
jeden search-sessions "query"
```

`export` emits `{id, path, state, ledgerVersion, activeLeaf,
recoveredTruncatedTail, events}` as JSON by default; `--markdown` and
`--html` render the same events. In the terminal, `/export [session]
[--format json|text] [--output file]` covers the slash side.

## The usage ledger: `.jeden/usage.json`

Every model completion appends one event to `<cwd>/.jeden/usage.json`. The
document is `{"version": 1, "updatedAt": <stamp>, "events": [...]}`, and
each event carries:

```json
{
  "at": "<unix-seconds>",
  "model": "<served model route>",
  "serviceTier": null,
  "inputTokens": 0.0,
  "outputTokens": 0.0,
  "cacheReadTokens": 0.0,
  "cacheWriteTokens": 0.0,
  "totalTokens": 0.0,
  "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
}
```

`cost` is present when the model is priced in the Brama catalog; prices are
per-million-token rates applied to each token class. When subscription
routing served the request, the event additionally carries a `billing`
object with `providerId`, `accountId`, `subscriptionId`, `quotaBucket`, and
`decisionId` — the audit trail for [model-access](model-access.md).

Readers of this file: `/usage show` aggregates calls, tokens, and cost;
`/usage reset` clears all recorded events; `jeden stats` (`--json`,
`--summary`, or `--serve` on `127.0.0.1`) reports the project ledger
alongside the user-scope `~/.jeden/usage.json`; and the interactive status
line sums the same file.

## The mode ledger: `.jeden/mode-state.json`

`<cwd>/.jeden/mode-state.json` is the project's durable mode state. As the
code defines it (`ModeState` in `rust/slash/state.rs`), the document is:

| Key | Shape | Meaning |
|---|---|---|
| `plan` | `{enabled, latestPlan}` | plan mode and the last recorded plan text |
| `goal` | `{enabled, paused, objective, budget, auto}` | the pinned durable objective; `auto` lets Oko's goal-lifecycle model start and finish goals from classified prompts |
| `guidedGoal` | `{active, roughObjective}` | guided-goal capture state |
| `loop_mode` | `{enabled, remaining, until, prompt}` | continuation loop |
| `fast` | `{enabled, serviceTier}` | fast mode; `serviceTier` defaults to `"priority"` and feeds the model request's service tier |
| `advisor` | `{enabled, model, lastReview}` | advisor review state |
| `force` | `{tool, prompt}` or `null` | forced next tool |
| `lastFailedTask`, `lastTask` | strings | last submitted task texts |
| `compact` | bool | compact rendering |
| `shake` | string | UI shake state |
| `todos[]` | `{text, status, createdAt}` | project todos |
| `branches[]` | `{id, title, createdAt, path, roadmapItem}` | session branches |
| `activeRoadmapItem` | string or `null` | pins session artifacts and branches to a roadmap item |
| `lastSessionPath` | path or `null` | most recent session directory |
| `tools` | `{approvalMode, approval{}}` | approval-policy overrides |

Writers serialize through a `.jeden/.mode-state.lock` file (create-new with
the writer's pid, 10 ms retry, bounded wait), then write a temporary
`.mode-state.json.tmp-<pid>-<nonce>`, flush, `fsync`, rename it over the
document, and sync the directory — a crash leaves the previous document
intact. Readers treat a missing or unparsable file as the default state.
Beyond the slash commands (`/plan`, `/goal`, `/loop`, `/todo`), the file
feeds the model request's service tier when `fast.enabled` is true and
stamps `activeRoadmapItem` onto new session artifacts.

Two more per-project files sit beside these:
`.jeden/subscription-cooldowns.json`, the durable `Retry-After`-bounded
cooldown store for subscription routing (see
[model-access](model-access.md)), and `.jeden/config.json`, project
configuration (see [configuration](configuration.md)).
