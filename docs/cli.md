# CLI reference

Every `jeden` command: arguments, flags, output shape, exit code, and exact
error sentences. Ground truth is `rust/main.rs` (parser and dispatch) plus
the per-command modules named below; the released vocabulary is frozen in
`released-surface.json`. Exit codes are uniform: `0` on success, `1` on any
error (errors print `Error: <message>` on stderr), with the two documented
exceptions at [doctor](#jeden-doctor---json---cwd-path) and
[conformance](#jeden-conformance---json---cwd-path).

## Global parsing rules

```
jeden [command] [flags] [positionals]
```

- No command (or a leading `--flag`) opens the interactive terminal.
- `--version`/`-V` prints `jeden <version>` (captured:
  `jeden 0.1.1+dev.356.9b82942`); `--help`/`-h` prints the usage text.
- Flags accepted everywhere they apply: `--cwd <path>`, `--model <name>`,
  `--max-tokens <n>`, `--max-steps <n>`, `--allow-write`, `--allow-command`,
  `--yolo`/`--auto-approve` (implies both allows), `--model-only`, `--json`,
  `--resume-session <path>`.
- Unknown `--flags` are refused with `unknown option: <flag>` — except after
  `export`, `roadmap`, `worktree`, `token`, `stats`, `gallery`, and after the
  task in `run`, where they pass through as positionals to the subcommand.
- `resume`, `recall_conversation`, `recall-conversation`, `search-sessions`,
  and `probierz` take their remaining argv verbatim (flag parsing is
  deliberately skipped; `resume` re-parses `--allow-write`,
  `--allow-command`, `--yolo` itself).
- `run`/`pursue` with no task: `run requires a task` / `pursue requires a
  task`. An unknown first word: `unknown command: <word>`.
- Before dispatch, environment files load in order `<cwd>/.env`,
  `.env.local`, `.env.production`, `.env.vercel`, `~/.jeden/.env` — each
  sets only variables still unset (see
  [configuration](configuration.md)). A load failure exits with
  `Error: failed to load environment files: <error>`.

## Running work

### `jeden` (interactive)

Opens the terminal UI in the checkout. All modes, slash commands, and views
live here; `/help` and the usage text list them. `--resume-session <path>`
reopens a recorded session.

### `jeden run "task"`

One-shot: run one task to completion, print the final answer on stdout, and
record the session (`rust/agent/commands.rs`).

- The session path is printed to stderr as `[session] <path>`.
- `--json` → pretty-printed
  `{"ok": true, "repaired": false, "originalError": null, "text": ...,
  "sessionPath": ...}`.
- `--model-only` → no tools, no session directory; plain text, or with
  `--json` the compact `{"ok": true, "text": ...}`.
- A leading `/command` is executed instead of a model turn: builtins are
  handled locally; `/compact`, `/handoff`, `/context`, `/checkpoint`,
  `/rewind` open the last durable conversation (refusal when none:
  `No prior session found. Run a task first, then use a session command.`);
  `/force <tool> <prompt>` arms the tool then runs the prompt (unknown tool:
  `Unknown or unavailable tool: <t>. Visible tools: <list>`); file-based
  custom commands expand to a prompt; anything else forwards to the model
  literally.
- Loop mode resubmits bounded continuations, separated by
  `— loop resubmit —` / `— loop resubmit failed —`.
- Offline (no `BRAMA_URL`): exit 1,
  `Error: BRAMA_URL is required; configure the Brama model-router service
  URL` — and the refusal is still ledgered; see
  [walkthrough-offline-refusals](walkthrough-offline-refusals.md).

### `jeden pursue "rough objective"`

Autonomous objective pursuit through Pursuit's accepted execution stages
(`rust/autonomy.rs`). Same grant flags as `run`. Every run writes a receipt
even on refusal — captured offline:

```
Error: BRAMA_URL is required; configure the Brama model-router service URL; receipt: <cwd>/.pursuit/runs/<stamp>/receipt.json
```

### `jeden resume <session-id-or-path> ["task"]`

Loads a recorded session's turns into a fresh conversation; with a task it
continues with a real turn (a genuine in-process resume). Accepts
`--allow-write`, `--allow-command`, `--yolo` among trailing args.

## Machine interfaces

### `jeden rpc`

Newline-delimited JSON on stdio; no socket. The full protocol —
banner, methods, events, interaction, errors — is [rpc](rpc.md).

### `jeden acp`

ACP on stdio for editors; maps the same session events minus goal events
(see [rpc](rpc.md#siblings-acp-and-headless)).

### `jeden headless <addr> <server-cert.pem> <server-key.pem> <client-ca.pem> <identity-map.json> [revoked-serials.txt]`

Opt-in mutual-TLS listener serving `jeden.session.v1`; wrong arity refuses
with the usage line above verbatim. Wire reference: [headless](headless.md);
isolation model: [concepts/headless-tenant](concepts/headless-tenant.md).

### `jeden collab-relay [addr]`

Local collaboration relay; default address `127.0.0.1:8877`.

## Inspecting sessions

Detail and captured output for all of these:
[walkthrough-session-export](walkthrough-session-export.md).

| Command | Output | Notable errors |
|---|---|---|
| `jeden sessions [limit]` | one directory name per line; `No sessions found.` when empty | — |
| `jeden show <id-or-path>` | the export JSON on stdout | missing id → `{"error": "session not found: <path>"}` (exit 0 — the error is the JSON); no id → `show requires a session id` |
| `jeden export <id-or-path> [output] [--markdown\|--html]` | JSON by default; with `output`, writes the file and prints its name | `export requires a session id or path`; internal format guard `unsupported session export format: <f>` |
| `jeden artifacts <id-or-path>` | `name\tbytes` per artifact; empty output when none | `artifacts requires a session id` |
| `jeden artifact <id-or-path> <name> [output]` | artifact content, or writes `output` | `artifact requires a session id or path`, `artifact requires an artifact name`, `artifact path escapes session: <name>` |
| `jeden search-sessions "query" [limit]` | `id\tts\ttype\tsnippet` per matching session, newest first | `search-sessions requires a query`, `search-sessions requires a non-empty query`, `cannot search session <path>: <error>` |
| `jeden recall_conversation <id-or-path>` | markdown transcript (also `recall-conversation`) | — |

## Configuration and health

### `jeden config [list|path|get <key>|set <key> <value>|reset <key>] [--json] [--cwd path]`

Typed settings registry (`rust/cli/config/schema.rs`). `list` prints every
key with type, current value, and description; `path` prints the user config
path (`~/.jeden/config.yml`). Errors, verbatim:
`Usage: jeden config [list|path|get <key>|set <key> <value>|reset <key>] [--json]`,
`config get requires a key`, `config set requires a key`,
`config set requires a value`, `config reset requires a key`,
`unknown config key: <key>`, and per-type validation such as
`<key> expects a boolean (true/false, yes/no, on/off, 1/0)`,
`<key> expects a finite number`, `<key> must be one of: <values>`,
`<key> expects a JSON array`. Note that file-only keys like `model` are not
in the registry: `jeden config set model x` refuses with
`unknown config key: model` — see [configuration](configuration.md#file-only-keys).

### `jeden doctor [--json] [--cwd path]`

Per-service health JSON: `{schemaVersion: 1, healthy, cwd, probes[]}` with
probes `brama`, `weles`, `storage`, `process`, `mcp`, `extensions`, `lsp`,
`browser`, `task`, `memory`, `collab`, `tui-keymap`; each probe carries
`state` (`healthy` | `degraded` | `unavailable`), `active`, `latencyMs`,
`detail`, and optional `evidence`. **Exit 0 only when no probe is
`unavailable`** — degraded still passes. Captured offline: `brama`
unavailable with detail `BRAMA_URL is required; configure the Brama
model-router service URL`, `weles` unavailable with
`WISENT_PLATFORM_BILLING_URL is not configured`, exit 1.

### `jeden conformance [--json] [--cwd path]`

Canonical JSON conformance report (`{schemaVersion: 2, complete, areaCount,
...}`); **exit 0 only when `complete` is true**.

### `jeden capabilities [--json] [--cwd path]`

Capability-registry snapshot; see
[concepts/capability](concepts/capability.md) for the captured output.

## Accounting and credentials

### `jeden stats [--json|--summary|--serve [--port N]]`

Project and user [usage ledgers](concepts/usage-record.md), quota, and
recent sessions. `--summary` is one line
(`0 events · 0 tokens · cost 0 · sessions 2`); `--serve` binds a dashboard
on `127.0.0.1`.

### `jeden token [--list] [--reveal] [--json]`

Prints the agent's Brama credential for scripting — redacted by default,
`--reveal` for the bare secret, `--json` for machine use. Refusals:
`BRAMA_URL is required; configure the Brama model-router service URL`, and
`WISENT_APP_AGENT_AUTH_SECRET is not configured; launch with bin/jeden-rust
or scripts/run-with-stado.sh`. The `/token` slash form never reveals,
because transcript text can reach the model.

## Maintenance

### `jeden update`

Verifies and applies a DSSE release manifest. Requires
`JEDEN_UPDATE_MANIFEST`; without it (captured):
`Error: JEDEN_UPDATE_MANIFEST must point to an HTTPS or local DSSE release
manifest`. Channel from `JEDEN_UPDATE_CHANNEL` (`stable` default).

### `jeden worktree [list|clear] [--dry-run] [--json] [--cwd path]`

Lists or clears jeden-managed git worktrees. Captured outside a repository:
`no jeden-managed git worktrees found (the task runtime prefers clone-based
isolation and only falls back to ``git worktree add --detach``;
clone-isolated workspaces are not worktrees); <cwd> is not inside a git
repository`.

### `jeden completions <bash|zsh|fish>`

Prints a shell completion script. Wrong shell:
`unknown shell '<s>': usage: jeden completions <bash|zsh|fish>`.

### `jeden tools [--json]`

Lists every registered tool with its one-line contract (name, jail, caps,
grant requirement) — the same registry the model sees.

### `jeden roadmap <list|show|add|drop|start|implemented|block|pass|status|depends|undepends|graph|acceptance|check|render|work> [args]`

Project roadmap store under version control; `--json` on every subcommand.

### `jeden probierz [args...]`

Runs Probierz discovery, evidence, and gate commands for Jeden.

### `jeden gallery [--theme NAME|--all] [--color]`

Renders TUI components across themes (dev tool).
