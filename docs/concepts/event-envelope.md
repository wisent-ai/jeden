# Event envelope

What exactly is one line of a session transcript? A `SessionEventV2`
envelope (`rust/session/event.rs`): a sequenced, parent-linked,
checksum-sealed JSON object around a payload from a closed vocabulary.
Everything a session ever did is one of these.

## Fields

Serialized camelCase, one JSON document per `transcript.jsonl` line:

| Field | Type | Meaning |
|---|---|---|
| `eventId` | string | `event-<timestamp>-<12-char alphanumeric suffix>`, unique within the ledger |
| `sessionId` | string | must match the session's `state.json` id |
| `parentId` | string or null | the previous active leaf; `null` only for the first event |
| `sequence` | number | 1-based position; must be exactly `len + 1` |
| `timestamp` | string | unix seconds |
| `causationId` | string or null | must equal `parentId` |
| `correlationId` | string | inherited from the previous event; the first event uses its own id; must be non-empty |
| `schemaVersion` | number | 2 (`SESSION_EVENT_SCHEMA_VERSION`) |
| `payload` | object | `{"type": <kind>, "data": <value>}` from the closed vocabulary below |
| `outbox` | array | exactly four pending [outbox](outbox.md) seeds, one per consumer |
| `checksum` | string | hex SHA-256 over the event serialized with an empty checksum field |

## Seal and verify

`seal()` clears the checksum field, serializes the event, and stores the
SHA-256 hex digest. `verify()` re-computes it on every read and refuses:

- `unsupported session event schema version <n>` — any version other than 2;
- `event <eventId> checksum mismatch` — any byte of the envelope changed
  after sealing.

Because the checksum covers `parentId`, `sequence`, and the outbox seeds,
an edited transcript line cannot pass replay.

## The closed payload vocabulary

`SessionPayloadV2` is a Rust enum: a variant is added to the source before
any producer can persist it, so misspelled event kinds cannot enter replay.
A legacy line with an unknown kind is refused with
`unsupported session event type: <kind>`.

The 56 kinds, grouped:

| Group | `payload.type` values |
|---|---|
| Conversation | `message`, `user`, `assistant`, `assistant_raw`, `final` |
| Tools | `action`, `tool_call`, `tool_result`, `approval`, `artifact` |
| Context | `context_snapshot`, `compaction`, `auto_compaction`, `auto_compaction_error`, `auto_continue`, `tool_prune`, `handoff` |
| Lineage | `lineage`, `branch`, `checkpoint`, `rewind` |
| Goals | `goal_lifecycle` |
| Memory | `memory_mutation`, `memory_recall` |
| Roadmap | `roadmap_item_created`, `roadmap_item_updated`, `roadmap_item_started`, `roadmap_item_blocked`, `roadmap_evidence_attached`, `roadmap_item_passed`, `roadmap_item_dropped` |
| Model routing | `model_attempt`, `model_route`, `model_route_result`, `model_retry`, `model_usage`, `usage_error` |
| Capabilities | `capability_generation` |
| Workers | `worker_job`, `worker_attempt`, `worker_lease`, `worker_event` |
| Collaboration | `collaboration`, `interaction` |
| Telemetry | `telemetry_reference` |
| Outcomes | `terminal_outcome`, `run_error` |
| Advisor / agents | `advisor`, `agent`, `agent_state` |
| Pending mutations | `pending_preview`, `pending_claim`, `pending_apply`, `pending_discard`, `pending_expire` |

(The legacy alias `roadmap_acceptance_updated` migrates to
`roadmap_item_updated`.)

Two payloads have typed shapes the validator inspects:

- `checkpoint` — `{"label": <string or null>, "messages": [...]}`: the exact
  model-visible context at the moment of the checkpoint.
- `rewind` — `{"checkpointId": ..., "fromLeafId": ...}`: the lineage jump,
  validated against the ledger (see [ledger](ledger.md)).

## A real envelope

Captured from an offline `jeden run` (no model key present):

```json
{"eventId":"event-1787608964-yYoyZNoQfYNA","sessionId":"1787608963-JqlaM8",
 "parentId":"event-1787608964-2EViiSwzwI8I","sequence":2,
 "timestamp":"1787608964","causationId":"event-1787608964-2EViiSwzwI8I",
 "correlationId":"event-1787608964-2EViiSwzwI8I","schemaVersion":2,
 "payload":{"type":"run_error","data":{"message":"BRAMA_URL is required; configure the Brama model-router service URL"}},
 "outbox":[{"consumer":"memory","eventId":"event-1787608964-yYoyZNoQfYNA","idempotencyKey":"session-event:event-1787608964-yYoyZNoQfYNA:memory","attempt":0,"leaseUntil":0,"state":"pending"}, "…three more consumers…"],
 "checksum":"…hex sha-256…"}
```

## Not to be confused with

- **The [ledger](ledger.md)** — the append/validate protocol between
  envelopes; this page is one envelope's anatomy.
- **`SessionEventV1`** — the headless replay wire event
  (`rust/rpc/replay.rs`), a different, connection-scoped object; see
  [headless](../headless.md).
- **RPC `session/event` notifications** — ephemeral wire frames derived from
  the run loop, not persisted envelopes; see [rpc](../rpc.md).
