# Runbook

Something refused, retried, or hung — what does the sentence mean, and what
do you check next? Each entry starts from the symptom and quotes the exact
string the source produces. First command for anything unclear:

```sh
jeden doctor
```

— per-service JSON with each probe's own sentence in `detail`; exit 0 only
when no probe is `unavailable` ([cli](cli.md#jeden-doctor---json---cwd-path)).

## The run refuses before any model call

These are configuration refusals from `rust/agent/runtime/routing.rs` and
`rust/model_router.rs`. Nothing was sent; fix the environment and rerun.

| Sentence | Meaning / repair |
|---|---|
| `BRAMA_URL is required; configure the Brama model-router service URL` | No Brama endpoint. Run `/setup` or export `BRAMA_URL` (fallback name: `STADO_MODEL_ROUTER_URL`). |
| `BRAMA_TOKEN is required; obtain the scoped Jeden model-router credential` | This Brama demands a bearer. Export `BRAMA_TOKEN` (fallback: `STADO_MODEL_ROUTER_TOKEN`). |
| `WISENT_APP_AGENT_AUTH_SECRET is required` | The HMAC signer has an empty secret. Launch through `bin/jeden-rust` or `scripts/run-with-stado.sh`, or export it; it is read from the environment only. |
| `no model selected; choose a model advertised by Brama; run /setup to configure` | Neither `--model`, config `model`, nor `JEDEN_MODEL` is set. |
| `model `<id>` is not in the Brama catalog` | The route id is not advertised to this signed agent. `GET /v1/models` scope differs per agent — check `/models`. |
| `model `<m>` is ambiguous; it matches multiple Brama routes: <list>; use the full route id` | A bare id suffix-matched more than one catalog route. |
| `model `<m>` is unavailable: <reason>` | The catalog itself marks the route unavailable (default reason: `catalog marks route unavailable`). |
| `modelRouting.retry.maxAttempts must be an integer from 1 through 8` (and siblings: `modelRouting.retry must be an object`, `modelRouting.<key> must be an array`, `modelRouting.<key> exceeds the 16-route limit`, `modelRouting.<key>[<i>].model must be a non-empty string`, `modelRouting.<key>[<i>].serviceTier must be a non-empty string`, `modelRouting.retry.jitterRatio must be between 0 and 1`, `modelRouting.retry.<key> must be a positive integer`) | Invalid `modelRouting` in [configuration](configuration.md); the config error is carried into the turn and fails it before any attempt. |

The refusal is also written into the session ledger as a `run_error` event —
`jeden search-sessions "BRAMA_URL"` finds every run that ever hit it
([walkthrough-offline-refusals](walkthrough-offline-refusals.md)).

## The turn retries, then fails

Retryable failures print one stderr line per scheduled retry, verbatim
`retry <attempt>/<retries> after <message>`, and record `model_retry`
ledger events. What retries and what does not
(`StreamErrorClass`, `rust/model_router.rs`):

- **Transient (retries, then falls through to the next route):** `Timeout`,
  `TransientHttp`, `Network`, `EmptyResponse`.
- **Terminal for the turn:** `Permanent`, `MalformedEvent`,
  `ContextOverflow` (promotes context routes instead), `Cancelled`
  (`Turn cancelled.`).
- **QuotaExhausted:** does not retry the same subscription — it cools it
  down and moves to the next target (below).
- **The visible-output rule:** once any model output reached the screen,
  nothing retries or fails over — the failure is surfaced as-is. Delivered
  tokens are never silently re-generated.

The attempt-level sentences:

| Sentence | Class | Meaning |
|---|---|---|
| `model router <status>: <first 800 bytes of body>` | by status | Non-2xx from Brama. 408/409/425/429/5xx are transient; a body containing `"retryable": false` is permanent (the gateway's own verdict wins); `provider_quota_exhausted`, HTTP 402, or a 429 mentioning `quota`/`subscription` is quota exhaustion; a 429 containing `subscription_unavailable` is transient (a reauth in progress clears on its own). |
| `model stream first-event timeout` | Timeout | Nothing arrived within `firstEventTimeoutMs` (default 30 s). |
| `model stream idle timeout` | Timeout | The stream stalled past `idleTimeoutMs` (default 45 s). |
| `model stream adapter disconnected` | Network | The transport thread died. |
| `model stream ended before [DONE]` | Network | EOF without the SSE terminator. |
| `model router returned no message` / `model router returned no message content` | EmptyResponse | Successful-but-empty response; deliberately transient — a retry can legitimately produce content. |
| `event-stream response arrived without SSE framing` | MalformedEvent | Content-type said stream, body was not. |
| context bodies matching `context length` / `context window` / `maximum context` / `too many tokens` / `tokens exceed` | ContextOverflow | Triggers `contextPromotions` route change, not a retry. |

Retry delay: `Retry-After` when the response names one (seconds or
HTTP-date), otherwise exponential `baseDelayMs · 4^(attempt−1)` capped at
`maxDelayMs` with `jitterRatio` (defaults: 3 attempts, 2 s base, 8 s cap,
0.2 jitter). All knobs: `modelRouting.retry` in
[configuration](configuration.md).

## Fallback and subscription rotation

Route order is: the selected route, then `modelRouting.fallbacks` (or the
catalog entry's own fallback list), bounded to 16 routes. Ledger
`model_route_result` events record each transition with these exact
reasons:

- `RetryScheduled` — reason is the attempt's error sentence.
- `RouteChanged` — `subscription targets and transient attempts exhausted`
  (or the image-capability refusal below when a vision-incapable route is
  skipped).
- `SubscriptionChanged` —
  `subscription quota or transient attempts exhausted`.

Subscription-specific sentences:

| Sentence | Meaning |
|---|---|
| `no eligible subscription target for model '<m>'` | Every discovered subscription for this route is cooling down or lacks the `chat` capability. Check `.jeden/subscription-cooldowns.json` (`{"version":1,"entries":[{"target":...,"untilMs":...}]}`) — a cooled entry only extends, never shortens. |
| `cannot open subscription cooldown store: <message>` | The cooldown file is unreadable/corrupt. Its own strings: `unsupported cooldown store version <n>`, `cooldown store poisoned`, `cooldown path has no parent`. |
| `cannot persist subscription cooldown: <message>` | Recording a quota cooldown failed; the turn stops rather than hammer an exhausted subscription. |

A quota-exhausted attempt cools the target for `Retry-After` (default 60 s
when absent) and rotates to the next frozen target; the frozen order and the
served target land in the [usage record](concepts/usage-record.md)'s
`billing` object and the ledger's `model_route` events.

## Vision requests on a text route

``model `<m>` does not advertise image input support; choose an
image-capable model`` — the selected route is not in Brama's image-capable
set. The automatic route (`any`) upgrades itself to `any-vision-capable`
when images are attached; an explicit route does not. Mid-fallback, a
vision-incapable route is skipped with a `RouteChanged` event carrying the
same sentence. Pick a vision-capable route or drop the attachment.

## A session refuses to load or to append

From `rust/session/store.rs` — see [concepts/ledger](concepts/ledger.md)
for the full invariant table:

- `cannot append <dir>: transcript has a recovered truncated tail; resume
  into a child session` — a crash tore the final line. Reads still work;
  continue in a child via `jeden resume`. Exercised live in
  [walkthrough-session-export](walkthrough-session-export.md).
- `<path>:<line> is malformed JSON: <error>` or `... is not a valid V2
  event: ...` — corruption **before** the tail; the reader refuses the whole
  ledger. Restore the file; do not hand-edit around it.
- `<path>:<line> event <id> checksum mismatch` — a line was modified after
  sealing. Same repair: restore.
- `<path>:<line> breaks ledger sequence/lineage ...` — events were
  reordered or deleted. Restore.

## `jeden headless` panics at startup

Observed on this source revision (`9b82942`, 2026-08-24), immediately on
launch, before binding:

```
Could not automatically determine the process-level CryptoProvider from Rustls crate features.
Call CryptoProvider::install_default() before this point to select a provider manually, or make sure exactly one of the 'aws-lc-rs' and 'ring' features is enabled.
```

Meaning: two rustls crypto providers are compiled into the dependency graph
and the daemon never pins one, so `ServerConfig` construction aborts. There
is no operator-side repair; the stdio interfaces (`jeden rpc`, `jeden acp`)
are unaffected. The wire contract in [headless](headless.md) applies once a
build pins a provider.

## Mode state is stuck

`timed out waiting for mode-state lock <cwd>/.jeden/.mode-state.lock` — a
writer crashed while holding the lock file (it contains the holder's pid).
Verify the pid is gone, remove the lock file, rerun. The document itself is
safe: writers commit via temp-file + rename
([concepts/mode-state](concepts/mode-state.md)).

## Update refuses

`JEDEN_UPDATE_MANIFEST must point to an HTTPS or local DSSE release
manifest` — `jeden update` has no default source; point it at the release
manifest and choose `JEDEN_UPDATE_CHANNEL` (`stable` default).

## One-shot session commands refuse

`No prior session found. Run a task first, then use a session command.` —
`jeden run "/compact"` (and `/handoff`, `/context`, `/checkpoint`,
`/rewind`) operate on `lastSessionPath` from mode state; nothing has run in
this checkout yet.
