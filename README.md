# Jeden

Jeden is Wisent's private agent harness. It keeps Wisent's model routing, policy, terminal loop, and local tools under our control instead of inheriting a generic coding-agent policy stack.

## Design contract

Jeden separates four planes:

1. **Inference** — every model call goes through the Wisent model router using HMAC-signed OpenAI-compatible chat completions.
2. **Policy** — the harness prompt is short, local, and explicit: no unrequested tests, no unrequested docs, no silent substitution, no command execution unless enabled.
3. **Tools** — tools are a small allowlisted registry with path-jail enforcement. Writes and commands are gated by CLI flags.
4. **Run loop** — the model must emit strict JSON actions. Invalid JSON is a hard failure.

## Current scope

The private M1 version includes:

- `jeden` interactive terminal mode.
- `jeden run "task"` one-shot mode.
- Session logs and artifacts under `~/.jeden/sessions/<id>/`.
- Model calls through `MODEL_ROUTER_URL`, `WISENT_APP_AGENT_ID`, and `WISENT_APP_AGENT_AUTH_SECRET`.
- Default model `claude-code-subscription`.
- Filesystem tools: `list_dir`, `read_file`, `search_text`, `search_files`, `write_file`, `apply_patch`.
- Command tools: `run_command`, `list_package_scripts`, `run_package_script`. One-shot execution requires `--allow-command`; interactive mode asks for approval when the flag is absent.
- Git read tools: `git_status`, `git_diff`.
- Artifact tool: `save_artifact` writes into the active session artifact directory, not the workspace.
- Existing file writes and patches require the `sha256` returned by `read_file`.
- Interactive mode asks before executing writes or commands unless the matching `--allow-*` flag is passed.
- Shared hooks are loaded from `~/.shared-hooks/run-hook.mjs` for `user_prompt_submit`, `pre_tool_use:*`, `post_tool_use:*`, and `stop`.
- No tests are included; repository hooks and `npm run check` are the quality gate.

## CLI

```sh
jeden --cwd ../content-platform
jeden --cwd ../content-platform --allow-command
jeden sessions 20
jeden show <session-id-or-path>
jeden run "summarize src/lib/api/model-router-hmac.ts" --cwd ../content-platform
jeden run "create notes.txt with hello" --cwd /tmp/sandbox --allow-write
```

Required env for real model calls:

```sh
WISENT_APP_AGENT_AUTH_SECRET=...
MODEL_ROUTER_URL=https://model-router-1080673333190.us-central1.run.app
WISENT_APP_AGENT_ID=wisent-app
```

The CLI loads `.env`, `.env.local`, `.env.production`, and `.env.vercel` from the launch directory and from `--cwd` before calling the router. Existing shell variables win.


Hooks can be disabled for debugging with:

```sh
JEDEN_HOOKS=0 jeden run "..." 
```
## JSON action protocol

The model must answer with one JSON object.

Tool call:

```json
{"action":"tool","tool":"read_file","input":{"path":"package.json"}}
```

Command call, only when enabled:

```json
{"action":"tool","tool":"run_command","input":{"command":"npm run check","timeoutMs":30000}}
```

Package script call:

```json
{"action":"tool","tool":"run_package_script","input":{"script":"check","timeoutMs":60000}}
```


Git diff call:

```json
{"action":"tool","tool":"git_diff","input":{"path":"src/tools.js"}}
```


Artifact call:

```json
{"action":"tool","tool":"save_artifact","input":{"name":"notes.txt","content":"research notes"}}
```
Patch call, only when writes are enabled:

```json
{"action":"tool","tool":"apply_patch","input":{"path":"package.json","expectedSha256":"...","replacements":[{"old":"\"version\": \"0.1.0\"","new":"\"version\": \"0.1.1\""}]}}
```
Final:

```json
{"action":"final","text":"Done."}
```

Tool results are fed back into the next model turn. The loop stops on `final` or when `--max-steps` is reached.
