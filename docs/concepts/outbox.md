# Outbox

How do downstream consumers — memory, collaboration, telemetry, remote
replication — see every session event exactly once, even across crashes? A
transactional outbox (`rust/session/outbox.rs`): each ledger event is born
with four pending delivery seeds, and delivery state advances only through
an append-only transition log beside the transcript.

## What it is

Every [event envelope](event-envelope.md) carries `outbox`: exactly one
`OutboxItem` per consumer, validated on append and on read (a wrong set is
refused with `has invalid transactional outbox seeds`).

```json
{
  "consumer": "memory",
  "eventId": "event-1787608964-yYoyZNoQfYNA",
  "idempotencyKey": "session-event:event-1787608964-yYoyZNoQfYNA:memory",
  "attempt": 0,
  "leaseUntil": 0,
  "state": "pending"
}
```

- `consumer` — one of `memory`, `collaboration`, `telemetry`,
  `remote_replication` (`OutboxConsumer::ALL`).
- `idempotencyKey` — `session-event:<eventId>:<consumer>`, fixed for the
  item's lifetime; consumers deduplicate on it.
- `state` — `pending`, `leased`, `delivered`, or `dead_letter`.

## Where state lives

The seeds inside the transcript are immutable (they are covered by the event
checksum). All progress is recorded in `outbox.jsonl` in the same session
directory: one JSON transition per line, each naming the consumer, event id,
idempotency key, attempt, lease deadline, new state, and an optional error
(bounded to 512 characters). The effective state of an item is its seed
folded through every transition in file order.

Reading the transition log is strict:

- `<path> has a truncated transition tail` — the file does not end in a
  newline; the torn write must be repaired before delivery resumes.
- `<path>:<line> invalid outbox transition: <error>` — malformed JSON.
- `<path>:<line> references unknown event <id>` — a transition for an event
  the ledger does not contain.
- `<path>:<line> violates outbox ordering` — a changed idempotency key or a
  decreasing attempt counter.

## The claim protocol

Delivery is lease-based and crash-safe:

1. **Claim** (`claim`): the first item for the consumer that is `pending`,
   or `leased` with an expired lease, is taken. Its attempt increments, its
   lease is set to `now + lease_seconds` (minimum 1), and a `leased`
   transition is appended. An item whose attempts already reached
   `max_attempts` is instead written as `dead_letter` with the error
   `maximum delivery attempts reached`.
2. **Complete** (`complete`): appends a `delivered` transition. Completing
   an already-delivered item is a no-op — delivery is idempotent.
3. **Retry** (`retry`): appends a `pending` transition carrying the
   truncated error, returning the item to the queue.

Both terminal transitions verify the caller still owns the lease: a
mismatched state, attempt, or lease deadline is refused with
`stale outbox lease for <eventId>`; a transition for an item that has no
effective state is `outbox item not found for <eventId>`. All operations
serialize on a process-wide lock (`session outbox lock poisoned` if it was
poisoned by a panic).

`pending_count` reports how many items are deliverable right now — pending,
or leased past their deadline.

## Why it exists

The append of a session event and the intent to deliver it are one atomic
write: the seeds ride inside the checksummed event line. A consumer that
crashes mid-delivery leaves a lease that expires; a consumer that delivers
twice is absorbed by the idempotency key; a poison event dead-letters after
bounded attempts instead of blocking the queue behind it.

## Not to be confused with

- **The [ledger](ledger.md)** — history. The outbox is delivery bookkeeping
  about that history; deleting `outbox.jsonl` loses progress, not events.
- **The headless [idempotency store](headless-tenant.md)** — request-level
  dedup for tenants' prompts; this outbox is event-level dedup for local
  consumers.
