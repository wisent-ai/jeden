# Session

What is the durable unit of work in Jeden? A session: one recorded
conversation between an operator (or a program) and the run loop, stored as
a directory the operator owns. Every interface — the interactive terminal,
`jeden run`, `jeden rpc`, `jeden acp`, `jeden headless` — creates and appends
to the same session shape.

## What it is

A session is a directory `~/.jeden/sessions/<unix-seconds>-<6-char suffix>/`
(`JEDEN_SESSION_ROOT` overrides the root; `rust/main.rs`). It holds exactly
three things:

- `state.json` — session metadata, written first (`rust/agent/runtime/recorder.rs`):

```json
{
  "version": 2,
  "id": "1787608963-JqlaM8",
  "cwd": "/private/tmp/demo/project",
  "startedAt": "1787608963",
  "activeLeaf": "event-1787608964-yYoyZNoQfYNA",
  "lineage": null
}
```

  `version` is the ledger schema version (2). `activeLeaf` is the event id
  of the current lineage tip, mirrored atomically after every append.
  `lineage` is `{"parentSession": ..., "parentEntry": ...}` when the session
  was resumed from another, else `null`.
- `transcript.jsonl` — the append-only [ledger](ledger.md) of
  [event envelopes](event-envelope.md).
- `artifacts/` — oversized tool results persisted as files and replaced in
  the model loop with a compact reference; listed by `jeden artifacts` and
  read by `jeden artifact`.

## Lifecycle

1. **Create.** The first turn writes `state.json` and appends the first
   event. Even a failed offline run records itself: a `jeden run` without
   `BRAMA_URL` leaves a two-event session (`user`, then `run_error` with the
   exact refusal) — see
   [walkthrough-offline-refusals](../walkthrough-offline-refusals.md).
2. **Append.** Every turn appends sequenced, parent-linked, checksummed
   events; the ledger never rewrites history (legacy migration is the one
   exception, and it rewrites to an equivalent v2 document).
3. **Branch and rewind.** `/checkpoint` and `/rewind` grow the ledger as a
   tree; abandoned lineages stay on disk. See [ledger](ledger.md).
4. **Resume.** `jeden resume <session> "task"` opens the recorded turns into
   a fresh conversation and runs a real turn — a new session directory whose
   turns replay the parent's model-visible history. RPC `session/open` does
   the same for programs.
5. **End.** There is no deletion path: Jeden never expires or deletes a
   session transcript. RPC `session/dispose` only releases the in-process
   handle.

## Identity

Two id spaces exist on purpose:

- The **durable id** is the directory name (`1787608963-JqlaM8`); every
  event carries it as `sessionId`, and `state.json` must agree — an event
  from another session is refused with
  `<path>:<line> belongs to session <id>, expected <id>`.
- The **wire id** is server-assigned per connection (`session-1`,
  `session-2`, …) by `jeden rpc` and the headless daemon; the durable
  directory is returned beside it as `sessionPath`.

## Refusal sentences

From `rust/cli/sessions.rs` and `rust/session/store.rs`:

- `session not found: <path>` — `jeden show`/`export` on an id that has no
  directory under the session root.
- `cannot append <dir>: transcript has a recovered truncated tail; resume
  into a child session` — the ledger refuses appends after tail recovery.
- `<state.json path> has no session id` — `state.json` lost its `id` and the
  directory name could not stand in.

## Commands

```sh
jeden sessions [limit]
jeden show <session-id-or-path>
jeden export <session-id-or-path> [output.json] [--markdown|--html]
jeden artifacts <session-id-or-path>
jeden artifact <session-id-or-path> <name> [output]
jeden resume <session> "continue"
jeden search-sessions "query"
jeden recall_conversation <session-id-or-path>
```

Full flag detail is in [cli](../cli.md); the end-to-end tour is
[walkthrough-session-export](../walkthrough-session-export.md).

## Not to be confused with

- **The [ledger](ledger.md)** — the append protocol inside a session; the
  session is the directory around it.
- **The [usage record](usage-record.md)** — per-project accounting in
  `<cwd>/.jeden/usage.json`, keyed by project, not by session.
- **The [headless tenant](headless-tenant.md)** — an isolation boundary that
  owns many sessions under `jeden headless`.
