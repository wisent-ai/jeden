# Headless service reference

`jeden headless` turns the harness into a listening, multi-tenant service:
mutual TLS in, the versioned `jeden.session.v1` envelope over
newline-delimited JSON inside. This page is the wire reference; the
isolation model is [concepts/headless-tenant](concepts/headless-tenant.md).
Ground truth: `rust/rpc/daemon.rs`, `rust/rpc/transport.rs`,
`rust/rpc/tls.rs`, `rust/rpc/service.rs`, and
`protocol/schema/v1/envelope.schema.json`.

## Starting it

```sh
jeden headless <addr> <server-cert.pem> <server-key.pem> <client-ca.pem> <identity-map.json> [revoked-serials.txt]
```

Wrong arity refuses with exactly that usage line. Startup refusals:
`failed to read identity map: <error>`, `invalid identity map: <error>`,
`identity map must not be empty`, `invalid identity mapping`,
`client CA bundle is empty`, `no certificate found in <path>`-class PEM
errors, `stored reconnect key is shorter than 32 bytes`, and
`failed to bind secure headless listener: <error>`. Durable state lands
under `<cwd>/.jeden/headless/` (`tenants/`, `idempotency/`, `replay/`,
`reconnect.key` created `0600`).

> Observed on this source revision (2026-08-24, `9b82942`): startup panics
> before binding with rustls' `Could not automatically determine the
> process-level CryptoProvider from Rustls crate features.` — see the
> [runbook](runbook.md#jeden-headless-panics-at-startup) for the exact
> symptom and meaning. The wire contract below is the code's contract and
> is enforced the moment the listener runs.

## Connection requirements

- **TLS 1.3 only** (`TLS 1.3 is required`); a plaintext or malformed first
  record is dropped (`plaintext or malformed TLS preface rejected`), and a
  slow preface hits `TLS preface deadline exceeded` (30 s).
- **ALPN must negotiate `jeden.session.v1`**
  (`required ALPN was not negotiated`).
- **A client certificate is required** (`client certificate is required`),
  must chain to the client CA bundle, must carry at least one URI or DNS SAN
  (`client certificate has no identity SAN`), must not be revoked
  (`client certificate is revoked`), and its SAN must resolve in the
  identity map (`certificate SAN is not mapped`).
- The trust material is reloadable in place; each connection records the
  `trust_generation` it was admitted under.
- Admission is bounded to 128 concurrent connections; the 129th receives a
  `backpressure` error (`connection admission capacity exhausted`,
  `retryAfterMillis: 100`) and is closed.

## Framing

One JSON document per line. Read timeout 30 s per frame
(`frame read deadline exceeded`), write timeout 10 s
(`frame write deadline exceeded`), maximum frame 1048576 bytes
(`frame exceeds 1048576 bytes`), empty lines refused (`empty frame`).
Framing errors answer `malformed_frame` (and close the connection);
unparsable JSON answers `malformed_json` (and continues).

## The envelope

Requests (`RequestEnvelopeV1`, camelCase):

```json
{"id": "r-1", "method": "session/prompt",
 "params": {"sessionId": "session-1", "prompt": "Respond exactly: OK"},
 "meta": {"protocolVersion": "jeden.session.v1",
          "idempotencyKey": "prompt-2026-08-24-001",
          "deadlineUnixMillis": null,
          "traceId": "trace-1"}}
```

- `meta.protocolVersion` must be `jeden.session.v1`; anything else answers
  `unsupported_protocol` — `protocolVersion must be jeden.session.v1`.
- `meta.deadlineUnixMillis`, when set and already past, answers
  `deadline_exceeded` — `request deadline has elapsed`.
- Responses are `{"id", "result"}`; errors are
  `{"id", "error": {code, message, retryable, details}}` — every error body
  carries an explicit `retryable` and a `details` object
  (`{"retryAfterMillis": n}` on the retryable ones).
- Missing/empty string params answer `invalid_request` — `missing <field>`.

## Methods

| Method | Params | Result |
|---|---|---|
| `health/readiness` (alias `readiness`) | — | `{state: "starting"\|"ready"\|"draining"\|"stopped"}` |
| `session/create` | — | `{sessionId, reconnectToken, expiresUnix}` |
| `session/reconnect` | `{reconnectToken}` | `{sessionId}` |
| `session/prompt` | `{sessionId, prompt}` + `meta.idempotencyKey` | `{state: "started", requestId}` or `{state: "reattached", requestId}` or `{state: "completed", requestId, result}` |
| `session/replay` | `{sessionId, requestId, cursor?, limit?}` | `{events: [...]}` |
| `session/cancel` | `{sessionId, requestId}` | `{cancelled: bool}` |

Anything else: `method_not_found` — `unknown session method`.

### Prompt semantics

`submit_prompt` (`rust/rpc/service.rs`) is idempotent and bounded:

1. An empty prompt answers `invalid_request` — `prompt must not be empty`.
2. The caller must own the session (`access_denied` otherwise — ownership is
   tenant equality, nothing weaker).
3. The idempotency key (≤512 bytes) is begun against the SHA-256 digest of
   the prompt: a repeat with the same digest **reattaches** to the running
   request or returns the **completed** cached result; a repeat with a
   different digest is a conflict (`idempotency_error`).
4. A per-tenant active-request permit is reserved (4 max;
   `quota_exceeded`, `retryAfterMillis: 250`).
5. The turn runs on a bounded executor (4 workers, queue 64). A full queue
   answers `backpressure` — `service capacity exhausted` with
   `retryAfterMillis`; a daemon that is not `ready` answers `not_ready` —
   `service is not ready` (`retryAfterMillis: 100`). Either way the
   idempotency record is rolled back so the key can be retried.
6. Every session event the turn emits is appended to the replay store; a
   turn that fails caches
   `{"error": {"code": "runtime_error", "message": ...}}` as its result —
   replaying the key returns the failure instead of re-running the prompt.

### Replay semantics

Events are durable per request, retained up to 10,000 per stream. `cursor`
is `cursor-<20-digit zero-padded sequence>` (e.g.
`cursor-00000000000000000005`); omitted means from the start. `limit`
defaults to 100, capped at 1000. Replayed events are the envelope-schema
`event` objects: `{type: "event", sessionId, streamId, sequence, cursor,
eventId, requestId, kind, payload, terminal}`. Errors surface as
`replay_error` with the store's own variant (invalid cursor, cursor ahead,
cursor older than retention, corrupt log line).

### Reconnect

`session/create` issues an HMAC-signed reconnect token bound to the
principal, tenant, and session, expiring after 300 s. `session/reconnect`
verifies it and returns the session id; a token from another identity or
past expiry answers `access_denied` — `invalid or expired reconnect token`.

## Error code table

| Code | retryable | Meaning |
|---|---|---|
| `unsupported_protocol` | false | wrong `meta.protocolVersion` |
| `deadline_exceeded` | false | request deadline already elapsed |
| `invalid_request` | false | missing/empty field, empty prompt |
| `access_denied` | false | unmapped identity, foreign session, bad reconnect token, invalid storage key |
| `quota_exceeded` | true | tenant limit hit; `details.retryAfterMillis` 250 (requests) or 1000 (sessions/bytes) |
| `backpressure` | true | executor queue or connection admission full |
| `not_ready` | true | daemon starting or draining |
| `idempotency_error` | false | key conflict or invalid key |
| `replay_error` | false | bad cursor or corrupt stream |
| `runtime_error` | false | the turn itself failed; message is the run loop's sentence |
| `method_not_found` | false | unknown method |
| `malformed_frame` / `malformed_json` | false | framing layer |
| `internal` | false | reconnect-token issuance failed |

## Shutdown

On shutdown signal the daemon stops accepting, drains the executor for up to
30 s (`graceful drain failed: <error>` if it cannot), then joins open
connections. Readiness reflects the drain (`draining`, then `stopped`).

## Client material

[examples/headless-mtls-material.sh](examples/headless-mtls-material.sh)
generates a demo CA, server certificate, client certificate with a mapped
SAN, and the identity map — the exact material this reference was verified
against.
