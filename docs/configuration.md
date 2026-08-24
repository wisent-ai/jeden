# Configuration

Where do Jeden's settings come from, and which layer wins? The rule is
short: the process environment wins over every file, project config wins
over user config, and `/setup` only ever appends non-secret keys. This page
covers the environment files, the config layers and their keys, and the
variables the source actually reads.

## Environment files

At startup Jeden loads, in order, `<cwd>/.env`, `.env.local`,
`.env.production`, `.env.vercel`, then `~/.jeden/.env`. Each file sets only
variables that are still unset, so earlier files and the real environment
always win. Values may be quoted, carry ` #` trailing comments, and use
`\n` escapes. `/setup` appends non-secret keys (`BRAMA_URL`,
`WISENT_APP_AGENT_ID`, model, preferences) to `~/.jeden/.env` at mode
`0600`; it never writes `WISENT_APP_AGENT_AUTH_SECRET`. `.env.example` at
the repository root documents the supported keys.

## Config layers

File configuration is layered and deep-merged, later layers overriding
earlier ones:

1. `~/.jeden/config.json` (legacy user location)
2. `~/.jeden/config.yml` (user)
3. `<cwd>/.jeden/config.json` (project)

Objects merge key-by-key; any non-object value replaces. `models` entries
merge by id across layers and `modelOverrides` extend, so a project can
price one model without redeclaring the catalog. Documents carry a schema
version (currently 3) and older documents are migrated in place through
recorded migration steps.

```sh
jeden config [list|path|get <key>|set <key> <value>|reset <key>] [--json] [--cwd path]
```

## Keys

| Key | Meaning |
|---|---|
| `model` | default model route; overridden by `--model` and consulted before `JEDEN_MODEL` |
| `agentId` | Brama agent id when `WISENT_APP_AGENT_ID` is unset |
| `authProviders` | per-provider auth configuration |
| `models[]` | `{id, cost{input, output, cacheRead, cacheWrite}}` local model catalog entries |
| `modelOverrides` | per-model `{cost}` overrides |
| `modelRouting` | `retry{maxAttempts, baseDelayMs, maxDelayMs, firstEventTimeoutMs, idleTimeoutMs, jitterRatio}`, `fallbacks[]`, `contextPromotions[]` of `{model, serviceTier?}` — see [model-access](model-access.md) |
| `context` | `maxBytes` (default 131072), `maxTokens` (default 32768) for loaded context files |
| `rules` | always-apply rule configuration |
| `secrets` | `mode` (default `redact`), `replacement` (default `[REDACTED]`), `minLength` (default 8), `values`, `environment`, `files`, `discoverEnvironment` (default true) — configured and discovered secret values are replaced in the model-bound context while the local transcript keeps the original |
| `billing` | `autoPurchaseEnabled` and `autoRenewEnabled` (both default false), `preferredCurrency`, `maxSingleMicrounits`, `maxPeriodMicrounits` |
| `ui` | `language` (one of the wisent-app locale codes; `JEDEN_LANGUAGE` wins), `theme` (default `auto`) |

## Environment variables

Model access (details in [model-access](model-access.md)):

| Variable | Meaning |
|---|---|
| `BRAMA_URL` | required Brama endpoint (`STADO_MODEL_ROUTER_URL` fallback name) |
| `BRAMA_TOKEN` | Brama bearer (`STADO_MODEL_ROUTER_TOKEN` fallback name) |
| `WISENT_APP_AGENT_ID` | signing agent id, default `wisent-app` |
| `WISENT_APP_AGENT_AUTH_SECRET` | HMAC signing credential, environment-only |
| `JEDEN_MODEL` | model route when neither `--model` nor config `model` is set |
| `JEDEN_SERVICE_TIER` / `MODEL_SERVICE_TIER` | request service tier, before `fast.serviceTier` from mode state |

State locations (details in [sessions](sessions.md)):

| Variable | Meaning |
|---|---|
| `JEDEN_SESSION_ROOT` | session directory root, default `~/.jeden/sessions` |
| `JEDEN_MEMORY_DB` | memory database path, default `~/.jeden/memory.sqlite3` (`JEDEN_MEMORY_FILE` remains an input-path override) |
| `JEDEN_TASK_STORE` | task/delegation store, default `<cwd>/.jeden/tasks` |
| `JEDEN_PLUGINS_HOME` | plugin root, default the home directory |

Other subsystems:

| Variable | Meaning |
|---|---|
| `JEDEN_UPDATE_MANIFEST` | HTTPS or local DSSE release manifest for `jeden update` (required by it) |
| `JEDEN_UPDATE_CHANNEL` | `canary` or `stable`, default `stable` |
| `JEDEN_LIFECYCLE_MODEL_URL` | override for Oko's loopback goal-lifecycle endpoint |
| `JEDEN_CONTEXT_LIMIT` | interactive context-size override |
| `JEDEN_LANGUAGE` | conversation language, wins over `ui.language` |
| `JEDEN_NODE` | node executable for the extension host, default `node`; the grant must permit the program |
| `JEDEN_TAMA_REGISTRY` | explicit Tama hook registry path |
| `ENTITLEMENTS_ROUTER_BIN` | local `entitlements-router` executable for auth status commands |
| `WISENT_PLATFORM_BILLING_URL` / `WISENT_PLATFORM_BILLING_TOKEN` | optional billing service (legacy aliases `WELES_URL`, `WELES_TOKEN` remain readable) |
| `STADO_MEDIA_ROUTER_URL` / `JEDEN_MEDIA_ROUTER_TOKEN` | optional media router for image and speech tools |
| `STADO_INTEGRATION_API_URL` / `JEDEN_STADO_INTEGRATION_TOKEN` | optional onboarding bundle reads and funnel events |

## Context files

Before each run Jeden loads user context from `~/.jeden/instructions.md` and
`~/.jeden/context.md`, then walks from the project ancestor to `--cwd`
reading `JEDEN.md`, `AGENTS.md`, `CLAUDE.md`, `RULES.md`,
`.jeden/instructions.md`, and `.jeden/context.md`. A context line
`@./extra.md` imports another file under the same context root; oversized
context files are skipped under the `context` limits above. File-based
custom commands load from project and user `.jeden/commands/`, native
extensions from `.jeden/extensions/`, custom tools from `.jeden/tools/`,
MCP servers from `~/.jeden/mcp.json` and `<cwd>/.jeden/mcp.json`, and
lifecycle hooks from `.jeden/hooks.json` (project hooks run only with
`--allow-command`).
