# Headless tenant

When `jeden headless` turns the harness into a listening service, who is the
caller — and what stops one caller from touching another's sessions? A
tenant: the isolation unit resolved from the client certificate, enforced by
`TenantGuard` (`rust/rpc/tenant.rs`) on every request.

## Identity: SAN → principal → tenant

The identity map passed on the command line
(`jeden headless <addr> <server-cert> <server-key> <client-ca>
<identity-map.json> [revoked-serials.txt]`) is a JSON array of:

```json
[{"san": "client.example", "principal": "docs-operator", "tenant": "docs-tenant"}]
```

- The map must not be empty (`identity map must not be empty`).
- `principal` and `tenant` ids are 1–255 bytes of
  `[A-Za-z0-9-_.:/]`; anything else is an invalid mapping.
- On each connection, the verified client certificate's URI SANs, then DNS
  SANs, are looked up in the map; the first match yields the
  `TenantPrincipal {principal, tenant}`. No match closes the connection
  (`certificate SAN is not mapped`). A certificate with no identity SAN at
  all is refused, as is one whose serial appears in the revocation list.

Authorization is then ownership: a session created by tenant A answers
`access_denied` to tenant B, always — `authorize_owner` compares tenants,
nothing else.

## Limits

`TenantLimits`, fixed by `serve_headless_cli` (`rust/rpc/server.rs`):

| Limit | Value | On excess |
|---|---|---|
| `max_active_requests` | 4 per tenant | `quota_exceeded`, `retryAfterMillis: 250` |
| `max_sessions` | 32 per tenant | `quota_exceeded`, `retryAfterMillis: 1000` |
| `max_stored_bytes` | 1 GiB per tenant | `quota_exceeded`, `retryAfterMillis: 1000` |

Active-request permits are RAII: a finished or crashed prompt releases its
slot when the permit drops. All quota errors are marked `retryable: true`
on the wire.

## Tenant storage

Every tenant's durable state lives under a hashed root:
`<cwd>/.jeden/headless/tenants/<sha256(tenant-id)>/` — workspaces are
`workspaces/<session-id>/`, artifacts resolve through `scoped_path`, which
refuses empty, absolute, or `..`-containing keys (`InvalidStorageKey`
surfaces as `access_denied`). A tenant id never appears as a raw directory
name.

Beside the tenant roots, the daemon keeps:

- `idempotency/<sha256(tenant)>/<sha256(key)>.json` — one durable record per
  idempotency key: `{version, tenant, key_digest, request_digest, state}`,
  where state is `active {request_id}` or `completed {request_id, result}`.
  A replayed key with the same request digest **reattaches** to the running
  request or returns the completed result; a different digest is a
  `Conflict`. Keys are ≤512 bytes; the digest is the SHA-256 of the prompt.
- `replay/` — the bounded per-request event log (10,000 events retained)
  that `session/replay` pages with `cursor-<20-digit>` tokens.
- `reconnect.key` — ≥32 random bytes, created `0600`; HMAC-signs reconnect
  tokens (`<base64url(claims)>.<base64url(hmac)>` with
  `{version, principal, tenant, session_id, expires_unix}`, TTL 300 s). A
  token presented by a different principal or tenant, or after expiry, is
  `access_denied: invalid or expired reconnect token`.

## Why it exists

`jeden rpc` trusts its parent process — stdio is the authorization. A
listening daemon cannot: the tenant is the unit that makes multi-client
service safe — identity from mTLS, storage under a hashed per-tenant root,
bounded concurrency and bytes, idempotent prompts, replayable events, and
resumable connections, all per tenant.

The full wire protocol — framing, methods, error bodies — is
[headless](../headless.md).

## Not to be confused with

- **A [session](session.md)** — a tenant owns many sessions; the session is
  the work, the tenant is the caller.
- **The local operator** — interactive, `run`, and `rpc` sessions have no
  tenant; they run as the operating-system user with the operator's own
  files.
- **The [outbox](outbox.md)** — event-level delivery bookkeeping inside a
  session; the tenant's idempotency store deduplicates whole prompts.
