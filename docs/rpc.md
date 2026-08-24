# The RPC protocol

How does a program — an editor extension, jeden-desktop, a CI job — drive
the same run loop a developer uses in the terminal? `jeden rpc` serves
newline-delimited JSON on stdio: requests go down, responses and event
notifications come back, and interactive questions and approvals become
protocol messages. This is the protocol the desktop consumes; goal-lifecycle
updates in particular reach long-lived subscribers over this session-event
stream (they deliberately have no ACP equivalent). Everything here is
defined in `rust/rpc/` and `rust/sdk/types.rs`.

## Transport and framing

- One JSON document per line, both directions. A frame larger than
  1048576 bytes is rejected with a `malformed_frame` error and the rest of
  the line is discarded; unparsable JSON answers `malformed_json`.
- `jeden rpc` is stdio-only and opens no socket.
- On start the server emits a banner before any request:

```json
{"type": "ready", "protocol": "jeden-rpc", "version": 1,
 "capabilities": {"protocolVersion": 1, "prompt": true, "abort": true,
   "resume": true, "eventSubscription": true, "elicitation": true,
   "approval": true, "transports": ["ndjson", "acp"]}}
```

## Requests and responses

A request is `{"id": ..., "method": "...", "params": {...}}`. Responses are
`{"id": ..., "result": {...}}` on success and `{"id": ..., "error":
{"code": "...", "message": "..."}}` on failure. Each method has a short and
a `session/`-prefixed alias:

| Method | Aliases | Params | Result |
|---|---|---|---|
| `initialize` | `capabilities` | — | `{protocol, capabilities}` |
| `session/new` | `new` | options (below) | `{sessionId, sessionPath}` |
| `session/open` | `session/load`, `resume` | `session` (id or path) + options | `{sessionId, sessionPath}` |
| `session/prompt` | `prompt` | `{sessionId, prompt, requestId?, goal?}` | `{requestId, text, sessionPath}` |
| `session/cancel` | `abort` | `{sessionId, requestId}` | `{aborted}` |
| `session/status` | `status` | `{sessionId}` | `{activeRequestIds}` |
| `session/dispose` | `dispose` | `{sessionId}` | `{disposed: true}` |
| `session/input_response` | `elicitation/resolve` | `{token, answer}` | `{accepted: true}` |
| `session/permission_response` | `approval/resolve` | `{token, approved}` | `{accepted: true}` |
| `shutdown` | — | — | `{shuttingDown: true}` |

Session options may be nested under `params.options` or inline in `params`:
`cwd`, `model`, `maxTokens`, `maxSteps`, `allowWrite`, `allowCommand`,
`autoApprove`. Session ids are server-assigned (`session-1`, `session-2`,
…); `sessionPath` is the durable directory documented in
[sessions](sessions.md).

Error codes: `method_not_found`, `invalid_params`, `unknown_session`,
`session_error`, `interaction_error`, `internal_error`, `prompt_failed`,
`malformed_frame`, `malformed_json`, `write_error`.

## The prompt lifecycle

`prompt` runs on its own worker thread, so the connection stays responsive:
`status`, `cancel`, and interaction responses are handled while a prompt is
in flight. `requestId` defaults to the wire `id` when omitted. `goal`, when
present, is the exact objective for the turn, kept separate from the
effective prompt.

While the prompt runs, the server pushes notifications (no `id`):

```json
{"method": "session/event", "params": {"requestId": "...", "kind": "..."}}
```

`kind` is one of `status` (`message`), `textDelta` (`text`), `elicitation`
(`token`, `question`, `options`), `approval` (`token`, `tool`, `detail`),
`goal` (`text`, `status` — `active` when Oko's classifier starts a goal,
`done` when it finishes one), `result` (`text`), and `error` (`message`).
Events are filtered by `requestId`; `result` and `error` are terminal for
the stream, and the final `{requestId, text, sessionPath}` response follows.

## Interaction: questions and approvals

When the agent needs the caller, the server sends a request-shaped
notification and blocks that tool call until the client answers or a
5-minute timeout expires:

```json
{"method": "session/request_input",
 "params": {"token": "...", "requestId": "...", "question": "...", "options": [...]}}
{"method": "session/request_permission",
 "params": {"token": "...", "requestId": "...", "tool": "...", "detail": "..."}}
```

The client answers with `session/input_response` (`{token, answer}`) or
`session/permission_response` (`{token, approved}`). An unknown token or a
mismatched interaction type answers `interaction_error`. On `shutdown` or
stdin close the server aborts active requests, cancels pending interactions
with `server shutting down`, joins its prompt workers, and disposes every
session before exiting.

## Siblings: ACP and headless

`jeden acp` serves ACP on stdio for editors that speak that protocol; it
maps the same session events, except goal events, which have no ACP
session-update equivalent — a desktop that wants them subscribes to this RPC
stream.

`jeden headless <addr> <server-cert.pem> <server-key.pem> <client-ca.pem>
<identity-map.json> [revoked-serials.txt]` is the opt-in listening variant:
mutual TLS, an identity map of `{san, principal, tenant}` entries, and an
optional revocation list. It layers a multi-tenant service on the same
session backend — per-tenant limits (4 active requests, 32 sessions, 1 GiB
stored), an idempotency store, an event replay store, and a reconnect key,
all under `<cwd>/.jeden/headless/`. Its wire envelope is the versioned
`jeden.session.v1` contract in `protocol/schema/v1/envelope.schema.json`.

The TypeScript (`packages/sdk-typescript`) and Python (`python/jeden_sdk`)
SDKs embed these machine interfaces.
