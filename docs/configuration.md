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

`get`/`set`/`reset` accept only the schema'd keys below; anything else
refuses with `unknown config key: <key>`. A value of the wrong shape
refuses with the key's own sentence: `<key> expects a boolean (true/false,
yes/no, on/off, 1/0)`, `<key> expects a finite number`, `<key> must be one
of: <values>`, `<key> expects a JSON array`, `<key> expects a JSON object`.
`reset` without a key: `config reset requires a key`. All executed against
`jeden 0.1.1+dev.356.9b82942`.

## Keys in the `jeden config` schema

What `jeden config list` prints, with type and default
(`rust/cli/config/schema.rs`):

| Key | Type (default) | Meaning |
|---|---|---|
| `tools.approvalMode` | enum `always-ask` \| `write` \| `yolo` (`always-ask`) | default approval policy for tool execution |
| `commands.enableClaudeUser` | boolean (true) | user slash commands from `~/.claude/commands` |
| `commands.enableClaudeProject` | boolean (true) | project slash commands from `.claude/commands` |
| `commands.enableOpencodeUser` | boolean (true) | user slash commands from `~/.config/opencode/commands` |
| `commands.enableOpencodeProject` | boolean (true) | project slash commands from `.opencode/commands` |
| `startup.showSplash` | boolean (false) | startup splash animation on normal launches |
| `startup.quiet` | boolean (false) | suppress startup chrome including the splash |
| `context.maxBytes` | number (131072) | max UTF-8 bytes loaded from discovered context and rule files |
| `context.maxTokens` | number (32768) | approximate token budget for discovered context and rule files |
| `rules.alwaysApply` | array ([]) | typed sticky rules injected into every rebuilt system prompt |
| `hooks.tamaRegistry` | string ("") | Tama hook registry path (shared-hooks `registry.json`); empty disables Tama hooks, unset auto-discovers |
| `secrets.mode` | enum `redact` \| `obfuscate` (`redact`) | protect known secrets in model-bound text |
| `secrets.minLength` | number (8) | minimum length for automatically discovered environment secrets |
| `secrets.discoverEnvironment` | boolean (true) | protect values of secret-named environment variables |
| `ui.language` | enum, 65 locale codes + `auto` (`auto`) | conversation language; `auto` follows the user's messages; `JEDEN_LANGUAGE` wins |
| `ui.theme` | enum `auto`, `graphite-dark`, `paper-light`, `titanium`, `nord`, `color-blind`, `mono`, `high-contrast`, `custom` (`auto`) | color theme; `custom` loads `.jeden/theme.json` |

## File-only keys

Read from the merged config document but not settable through
`jeden config set` — edit the file or let `/setup` append them:

| Key | Meaning |
|---|---|
| `model` | default model route; overridden by `--model` and consulted before `JEDEN_MODEL` |
| `agentId` | Brama agent id when `WISENT_APP_AGENT_ID` is unset |
| `authProviders` | per-provider auth configuration |
| `models[]` | `{id, cost{input, output, cacheRead, cacheWrite}}` local model catalog entries |
| `modelOverrides` | per-model `{cost}` overrides |
| `modelRouting` | `retry{maxAttempts, baseDelayMs, maxDelayMs, firstEventTimeoutMs, idleTimeoutMs, jitterRatio}`, `fallbacks[]`, `contextPromotions[]` of `{model, serviceTier?}` — see [model-access](model-access.md); invalid values fail the turn with the exact sentences in the [runbook](runbook.md#the-run-refuses-before-any-model-call) |
| `secrets.values` / `secrets.environment` / `secrets.files` / `secrets.replacement` | extra protected literals, named variables, secret files, and the replacement text (default `[REDACTED]`); the model-bound context is rewritten while the local transcript keeps the original |
| `billing` | `autoPurchaseEnabled` and `autoRenewEnabled` (both default false), `preferredCurrency`, `maxSingleMicrounits`, `maxPeriodMicrounits` |

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
