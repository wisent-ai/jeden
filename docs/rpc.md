# The RPC protocol

How does a program — an editor extension, jeden-desktop, a CI job — drive
the same run loop a developer uses in the terminal? `jeden rpc` serves
newline-delimited JSON on stdio: requests go down, responses and event
notifications come back, and interactive questions and approvals become
protocol messages. This is the protocol the desktop consumes; goal-lifecycle
updates in particular reach long-lived subscribers over this session-event
stream (they deliberately have no ACP equivalent). Everything here is
defined in `rust/rpc/server.rs`, `rust/rpc/interaction.rs`, and
`rust/sdk/types.rs`.

## Transport and framing

- One JSON document per line, both directions. Trailing `\r` and `\n` are
  trimmed from incoming frames.
- A frame larger than 1048576 bytes is rejected with a `malformed_frame`
  error (`frame exceeds 1048576 bytes`) and the rest of that line is
  discarded; the connection continues.
- Unparsable JSON answers `malformed_json` with the parser's message and a
  `null` id.
- `jeden rpc` is stdio-only and opens no socket. EOF on stdin begins
  shutdown.
- On start the server emits a banner before any request (captured):

```json
{"type":"ready","protocol":"jeden-rpc","version":1,"capabilities":{"protocolVersion":1,"prompt":true,"abort":true,"resume":true,"eventSubscription":true,"elicitation":true,"approval":true,"transports":["ndjson","acp"]}}
```

## Requests and responses

A request is `{"id": ..., "method": "...", "params": {...}}` — `id` may be
any JSON value and is echoed back. Responses are `{"id": ..., "result":
{...}}` on success and `{"id": ..., "error": {"code": "...", "message":
"..."}}` on failure. Each method has a short and a `session/`-prefixed
alias; both are exact:

| Method | Aliases | Params | Result |
|---|---|---|---|
| `initialize` | `capabilities` | — | `{protocol: "jeden-rpc", capabilities}` (the banner's capabilities object) |
| `session/new` | `new` | session options (below) | `{sessionId, sessionPath}` |
| `session/open` | `session/load`, `resume` | `session` (durable id or path) + session options | `{sessionId, sessionPath}` |
| `session/prompt` | `prompt` | `{sessionId, prompt, requestId?, goal?}` | `{requestId, text, sessionPath}` after the turn |
| `session/cancel` | `abort` | `{sessionId, requestId}` | `{aborted: bool}` |
| `session/status` | `status` | `{sessionId}` | `{activeRequestIds: [..]}` |
| `session/dispose` | `dispose` | `{sessionId}` | `{disposed: true}` |
| `session/input_response` | `elicitation/resolve` | `{token, answer}` | `{accepted: true}` |
| `session/permission_response` | `approval/resolve` | `{token, approved: bool}` | `{accepted: true}` |
| `shutdown` | — | — | `{shuttingDown: true}` |

Session options may be nested under `params.options` or inline in `params`
(`SessionOptions`, all optional): `cwd` (default: the server's current
directory), `model`, `maxTokens`, `maxSteps`, `allowWrite` (false),
`allowCommand` (false), `autoApprove` (false). Wire session ids are
server-assigned (`session-1`, `session-2`, …); `sessionPath` is the durable
directory documented in [concepts/session](concepts/session.md).

String parameters must be non-empty: a missing or blank required string is
`invalid_params` with `<key> must be a non-empty string`.

### Error codes

| Code | Produced by |
|---|---|
| `method_not_found` | unknown method — `unknown method: <name>` |
| `invalid_params` | missing/blank string params; unparsable session options (e.g. `invalid type: integer `123`, expected path string`) |
| `unknown_session` | `cancel`/`status`/`dispose`/interaction on an id this connection never created — `unknown session: <id>` |
| `session_error` | session backend failures (create, resume, dispose, abort) |
| `interaction_error` | bad interaction token or type (below) |
| `internal_error` | poisoned server state |
| `prompt_failed` | any prompt-turn failure; the message is the run loop's own sentence, e.g. `BRAMA_URL is required; configure the Brama model-router service URL` |
| `malformed_frame` / `malformed_json` | framing layer (null id) |
| `write_error` | the response could not be written |

## The prompt lifecycle

`prompt` runs on its own worker thread, so the connection stays responsive:
`status`, `cancel`, and interaction responses are handled while a prompt is
in flight. `requestId` defaults to the wire `id` when omitted. `goal`, when
present (non-blank), is the exact objective for the turn, kept separate from
the effective prompt.

While the prompt runs, the server pushes notifications (no `id`):

```json
{"method": "session/event", "params": {"requestId": "...", "kind": "...", ...}}
```

`kind` and its payload fields (`SessionEventKind`, internally tagged):

| `kind` | Fields | Meaning |
|---|---|---|
| `status` | `message` | progress line, e.g. `thinking (step 1/unbounded)` |
| `textDelta` | `text` | streamed answer fragment |
| `elicitation` | `token`, `question`, `options` | the agent asked the caller a question |
| `approval` | `token`, `tool`, `detail` | a gated tool call awaits permission |
| `goal` | `text`, `status` | Oko's classifier started (`active`) or finished (`done`) a goal |
| `result` | `text` | terminal: the turn's final text |
| `error` | `message` | terminal: the turn failed |

Events are filtered by `requestId`; after a terminal event the final
`{id, result: {requestId, text, sessionPath}}` (or `prompt_failed`) follows.
A captured offline exchange:

```json
{"id":2,"result":{"sessionId":"session-1","sessionPath":"/tmp/.../.jeden/sessions/1787608989-tig1z6"}}
{"method":"session/event","params":{"requestId":"3","kind":"status","message":"thinking (step 1/unbounded)"}}
{"method":"session/event","params":{"requestId":"3","kind":"error","message":"BRAMA_URL is required; configure the Brama model-router service URL"}}
{"id":3,"error":{"code":"prompt_failed","message":"BRAMA_URL is required; configure the Brama model-router service URL"}}
```

## Interaction: questions and approvals

When the agent needs the caller, the server sends a request-shaped
notification and blocks that tool call until the client answers or the
300-second interaction timeout expires (`rust/rpc/interaction.rs`):

```json
{"method": "session/request_input",
 "params": {"token": "...", "requestId": "...", "question": "...", "options": [...]}}
{"method": "session/request_permission",
 "params": {"token": "...", "requestId": "...", "tool": "...", "detail": "..."}}
```

The client answers with `session/input_response` (`{token, answer}`) or
`session/permission_response` (`{token, approved}`). Exact
`interaction_error` messages: `unknown interaction token: <token>`,
`interaction token is for elicitation` (an input token answered as an
approval), `interaction token is for approval` (the reverse).

On `shutdown` or stdin close the server aborts every session's active
requests, cancels pending interactions with `server shutting down`, joins
its prompt workers, and disposes every session before exiting.

## Siblings: ACP and headless

`jeden acp` serves ACP on stdio (agent name `jeden-acp`) for editors that
speak that protocol: `textDelta` maps to agent message chunks, structured
`status` payloads map to tool-call and plan updates, elicitations and
approvals surface as thought/tool chunks with the real interaction handled
by the ACP layer. Goal and error events deliberately have no ACP
session-update equivalent — a desktop that wants goal lifecycle subscribes
to this RPC stream.

`jeden headless` is the opt-in mutual-TLS listener speaking the versioned
`jeden.session.v1` envelope — a different wire contract with tenant
isolation, idempotent prompts, and event replay: see [headless](headless.md)
and [concepts/headless-tenant](concepts/headless-tenant.md).

The TypeScript (`packages/sdk-typescript`) and Python (`python/jeden_sdk`)
SDKs embed these machine interfaces. A runnable driver script is
[examples/rpc-drive.sh](examples/rpc-drive.sh).
