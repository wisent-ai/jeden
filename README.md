# Jeden

Jeden is Wisent's private coding-agent harness. Model routing, policy, terminal interaction, sessions, memory, tools, hooks, and MCP integration are implemented and controlled locally.

## Design contract

Jeden separates four concerns:

- **Inference** — model calls go through Brama using HMAC-signed OpenAI-compatible chat completions.
- **Policy** — the harness prompt and approval rules are explicit and local.
- **Tools** — a small allowlisted registry enforces path jails and write or command permission.
- **Run loop** — the model may return native tool calls or strict JSON actions that enter the same local execution loop.

Tool schemas are derived from each input contract and sent with the model request. Tool results are recorded in the session and returned to the model until it produces a final answer.

## Current scope

The private milestone includes:

- interactive terminal and one-shot `jeden run` modes;
- session transcripts and artifacts under `~/.jeden/sessions/`;
- model routing through `BRAMA_URL`, `WISENT_APP_AGENT_ID`, and `WISENT_APP_AGENT_AUTH_SECRET`;
- model selection through `--model`, `JEDEN_MODEL`, or native config;
- jailed filesystem, document, archive, image, SQLite, search, Git, process, evaluation, URL, artifact, memory, todo, delegation, and MCP tools;
- guarded file mutations using the digest or snapshot tag returned by `read_file`;
- custom JavaScript tools from `~/.jeden/tools/` and `<cwd>/.jeden/tools/`;
- project and user lifecycle hooks;
- native `.jeden` configuration, context, command, extension, plugin, memory, and mode-state paths;
- interactive approval for writes and commands unless explicitly enabled.

File mutations return Jeden-native visual diffs and previews. Oversized tool results are persisted as session artifacts and replaced in the model loop with a compact reference.

## Tool policy

- Discover files with `glob_paths` or `list_dir` before reading unknown paths.
- Search content with `grep_regex` or `search_files` rather than shell discovery commands.
- Use targeted `read_file` selectors instead of dumping large files.
- Retry with a narrower path or alternate pattern before concluding that a symbol is absent.
- Use `run_package_script` for declared package scripts and reserve general process tools for commands without a safer built-in.
- Verify behavior changes with the narrowest relevant check.

## CLI

```sh
jeden
jeden --cwd ../content-platform
jeden run "summarize package.json"
jeden run "create notes.txt" --allow-write
jeden run "inspect the build" --allow-command
jeden sessions
jeden show <session>
jeden export <session> <output>
jeden artifacts <session>
jeden artifact <session> <name> <output>
jeden resume <session> "continue"
jeden search-sessions "query"
jeden recall_conversation --list
jeden tools --cwd ../content-platform
jeden config --cwd .
jeden doctor --cwd .
jeden capabilities --cwd .
```

## Interactive terminal views

In a terminal, management commands without arguments open native searchable views instead of printing command syntax. This covers authentication, models, settings, approval policy, sessions, todos, modes, tools, MCP and SSH configuration, usage, memory, browser state, collaboration, jobs, extensions, plugins, and marketplaces. Selecting a row dispatches the same validated slash command that can still be entered directly.

Picker controls:

- type to filter labels, details, and status badges;
- `Up` and `Down` move the active row;
- `Home`, `End`, `PageUp`, and `PageDown` jump through the list;
- `Ctrl-U` clears the filter;
- `Enter` executes the selected action;
- `Esc` closes the current view.

Destructive rows open a confirmation view with **Cancel** selected by default. Move to **Confirm** and press `Enter` to execute. Agent `ask_user` calls use the same terminal-owned event loop: option questions open a picker and open questions accept free text without allowing a worker thread to read terminal input directly.

When stdin is not a terminal, interactive views render as deterministic text lists and direct slash arguments remain available for scripts.

Required environment for real model calls:

```sh
WISENT_APP_AGENT_AUTH_SECRET=<signing-credential>
BRAMA_URL=<brama-url>
WISENT_APP_AGENT_ID=wisent-app
```

`ENTITLEMENTS_ROUTER_BIN` optionally overrides the local `entitlements-router` executable used by authentication status commands.

## Billing and subscription routing

Billing is owned by Weles. Configure `WELES_URL` and `WELES_TOKEN`; Jeden never accepts or stores card numbers, CVC/CVV values, processor tokens, or addresses. `/payment-method setup --account <id>` only opens a Weles-hosted HTTPS setup URL.

The interactive slash surface provides:

- `/billing policy get|set|reset` for an explicit, revision-pinned purchase policy with product, currency, per-purchase, and per-period caps;
- `/subscriptions list|status` for redacted subscription and quota views;
- `/subscriptions purchase|renew|disable` for approved, caller-idempotent mutations.

`policy set` requires `--approve`; automatic purchase and renewal remain disabled until the Weles policy is explicitly enabled. Financial mutations require a caller-supplied idempotency key and are validated against pinned policy and quote revisions by Weles.

For model calls, Jeden discovers active Weles subscriptions and their quota snapshots. It freezes a deterministic order per logical request, sends the selected `billingTarget` to Brama, and preserves the same request, idempotency, and decision identities across attempts. A typed quota-exhaustion response moves the target into a durable, `Retry-After`-bounded cooldown and selects the next eligible subscription. Failover never occurs after model output has become visible. The served account, subscription, quota bucket, and decision ID are recorded in the session audit and usage ledger.

## Configuration and context

User config loads from `~/.jeden/config.json` and `~/.jeden/config.yml`. Project config loads from `<cwd>/.jeden/config.json` and overrides user config. Environment variables still win over file config.

Before each run, Jeden loads user context from `~/.jeden/instructions.md` and `~/.jeden/context.md`. Project context walks from the project ancestor to `--cwd` and reads:

- `JEDEN.md`
- `AGENTS.md`
- `CLAUDE.md`
- `RULES.md`
- `.jeden/instructions.md`
- `.jeden/context.md`

A context line such as `@./extra.md` imports another file under the same context root. Oversized context files are skipped.

File-based custom commands load from project and user `.jeden/commands/` directories. Native extensions load from project and user `.jeden/extensions/` directories. Plugin and marketplace state lives under `~/.jeden/plugins/`.

## Sessions and memory

`jeden export`, `show`, `artifacts`, `artifact`, `search-sessions`, `resume`, and `recall_conversation` inspect or reuse recorded work. Resumed work starts a fresh session seeded with the selected history.

Durable memory uses `~/.jeden/memory.jsonl` by default. `JEDEN_MEMORY_FILE` may override the location for isolated runs.

## MCP and hooks

MCP servers load from `~/.jeden/mcp.json` and `<cwd>/.jeden/mcp.json` using the standard `mcpServers` shape. Generic MCP tools list and call server tools, resources, and prompts; configured server tools may also appear under native `mcp__<server>__<tool>` names.

Shared lifecycle hooks receive user-prompt, pre-tool, post-tool, session-start, and stop events. Hook output may add context, replace supported input fields, or block an action through the documented decision contract.

## Custom tools

Custom modules export a default factory that receives the current workspace helpers and returns one tool or a list of tools. Tool names must be unique and cannot collide with built-ins. Custom execution remains subject to the same jail, approval, and hook policy as built-in tools.

List active tools with:

```sh
jeden tools --cwd .
```

## JSON action protocol

The complete native action, tool-call, selector, and anchored-patch contract is documented in [docs/JSON_ACTION_PROTOCOL.md](docs/JSON_ACTION_PROTOCOL.md).
