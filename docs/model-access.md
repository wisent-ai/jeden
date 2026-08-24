# Model access via Brama

How does Jeden reach a model? Only through Brama. The harness carries no
provider API key and no provider SDK; it POSTs OpenAI-compatible chat
completions to `BRAMA_URL` at `/v1/chat/completions`, signs every request
with a per-agent HMAC credential, and lets Brama own routing to actual
providers. Everything on this page is defined in `rust/model_router.rs`,
`rust/control_plane/brama.rs`, `rust/agent/runtime/routing.rs`, and
`scripts/run-with-stado.sh`.

## The signed request

Every Brama request carries four headers computed from the request body:

- `x-agent-id` — `WISENT_APP_AGENT_ID`, defaulting to `wisent-app`;
- `x-agent-timestamp` — unix seconds;
- `x-agent-body-sha256` — hex SHA-256 of the body (empty for an empty body);
- `x-agent-signature` — hex HMAC-SHA256 over `<agent-id>:<timestamp>:<body-sha256>`,
  keyed by `WISENT_APP_AGENT_AUTH_SECRET`.

The secret itself never leaves the process: it is read from the environment
only, never written to disk, and an empty secret fails with
`WISENT_APP_AGENT_AUTH_SECRET is required`. When Brama additionally demands
a bearer, `BRAMA_TOKEN` is sent as `Authorization: Bearer`. Missing values
are configuration errors with exact messages: `BRAMA_URL is required;
configure the Brama model-router service URL` and `BRAMA_TOKEN is required;
obtain the scoped Jeden model-router credential`. `STADO_MODEL_ROUTER_URL`
and `STADO_MODEL_ROUTER_TOKEN` are accepted as fallback names for the same
two values.

`jeden token` prints the agent's own Brama credential for scripting —
redacted by default, `--reveal` for the bare secret, `--json` for machine
use. The `/token` slash form never reveals, because transcript text can
reach the model.

## The credential rule

`scripts/run-with-stado.sh` (which `bin/jeden-rust` invokes automatically
when the model runtime is needed and credentials are absent) encodes the
rule: **a workload reads its own credential and nobody else's.** Each value
is a distinct Skarbiec item read under the consumer identity that holds a
grant for exactly that item:

| Environment value | Skarbiec item | Read as |
|---|---|---|
| `WISENT_APP_AGENT_AUTH_SECRET` | `agent:wisent-app` (field `value`) | vault directly when readable, else the managed CLI path |
| `BRAMA_TOKEN` | `jeden-model-router` (field `token`) | the operator's own consumer |
| `JEDEN_STADO_INTEGRATION_TOKEN` | `jeden-integration-api` (field `token`) | the dedicated `jeden-onboarding-client` consumer |

The agent capability is never reused for the bearer, and the onboarding
client has its own consumer and grant file — a consumer without the grant is
refused. Each credential lands in the process environment and nowhere else;
a missing optional item degrades that feature (a bearer-requiring Brama
refuses the run; the first-use journey stays offline) instead of borrowing
another identity's read. Rotation and revocation are owned by Skarbiec, not
Jeden.

## Model selection and the catalog

The model route is chosen in precedence order: `--model`, then the `model`
key from [configuration](configuration.md), then `JEDEN_MODEL`. Brama
advertises the catalog at `GET /v1/models`, filtered for the signed agent;
Jeden caches it in memory and on disk under `~/.jeden/cache/` keyed by
endpoint, bearer, and agent scope, so a fresh start reuses the matching
scoped catalog. A bare (provider-less) model id resolves to the unique
catalog route whose id ends with `/<model>`; an ambiguous id is an error
naming every matching route. No selected model is an error telling you to
run `/setup`.

## Retry, fallbacks, and tiers

Transient errors retry under `modelRouting.retry` (`maxAttempts` 1–8,
`baseDelayMs`, `maxDelayMs`, `firstEventTimeoutMs`, `idleTimeoutMs`,
`jitterRatio` 0–1). `modelRouting.fallbacks` and
`modelRouting.contextPromotions` are lists of `{model, serviceTier?}` route
descriptors, validated against the catalog; when unset, the selected
catalog entry's own `fallback` and `promotion` lists apply. The request's
service tier comes from `JEDEN_SERVICE_TIER`, then `MODEL_SERVICE_TIER`,
then `fast.serviceTier` from `.jeden/mode-state.json` when fast mode is
enabled. Neither retry nor failover happens after model output has become
visible.

## Subscriptions and the audit trail

When Wisent Platform Billing is configured, Jeden discovers active Weles
subscriptions, freezes a deterministic order per logical request, and sends
the selected billing target to Brama. A typed quota-exhaustion response
records a `Retry-After`-bounded cooldown in
`<cwd>/.jeden/subscription-cooldowns.json` and the next eligible
subscription is selected. Every completion appends tokens, the
catalog-priced cost breakdown, and the served billing target and decision id
to `<cwd>/.jeden/usage.json` — the ledger documented in
[sessions](sessions.md).
