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
- Filesystem tools: `list_dir`, `read_file`, `search_text`, `search_files`, `glob_paths`, `grep_regex`, `write_file`, `apply_patch`.
- Command tools: `run_command`, `list_package_scripts`, `run_package_script`. One-shot execution requires `--allow-command`; interactive mode asks for approval when the flag is absent.
- Git read tools: `git_status`, `git_diff`.
- Web read tool: `fetch_url`.
- Artifact tool: `save_artifact` writes into the active session artifact directory, not the workspace.
- Custom JavaScript tools auto-load from `~/.jeden/tools/*.js|*.mjs` and `<cwd>/.jeden/tools/*.js|*.mjs`.
- Session todo tool: `todo` stores task state under the active session artifacts directory.
- Existing file writes and patches require the `sha256` returned by `read_file`.
- Project context auto-loads from `JEDEN.md`, `AGENTS.md`, `CLAUDE.md`, `.jeden/instructions.md`, and `.jeden/context.md` under `--cwd`.
- Interactive mode asks before executing writes or commands unless the matching `--allow-*` flag is passed.
- Shared hooks are loaded from `~/.shared-hooks/run-hook.mjs` for `user_prompt_submit`, `pre_tool_use:*`, `post_tool_use:*`, and `stop`.
- No tests are included; repository hooks and `npm run check` are the quality gate.

## CLI

```sh
jeden --cwd ../content-platform
jeden --cwd ../content-platform --allow-command
jeden resume <session-id-or-path> "continue with the previous context" --cwd ../content-platform
jeden sessions 20
jeden show <session-id-or-path>
jeden tools --cwd ../content-platform
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

Before each run, Jeden appends project context files from `--cwd` to the system prompt when present. Oversized context files are skipped.



Use `jeden resume <session-id-or-path> "task"` to seed a new run with the prior session transcript summary while recording a fresh session.
Hooks can be disabled for debugging with:

```sh
JEDEN_HOOKS=0 jeden run "..." 
```

## Custom tools

Custom tool modules export a default factory. The factory receives `{ cwd, exec, readText }` and returns one tool or an array of tools:

```js
export default (jeden) => ({
  name: 'repo_name',
  description: 'Return package name from package.json',
  input: {},
  async execute() {
    const pkg = JSON.parse(await jeden.readText('package.json'))
    return { name: pkg.name }
  },
})
```

Discovery order is user tools first, then project tools. Tool names must be unique and cannot collide with built-ins. List active tools with:

```sh
jeden tools --cwd .
```
## JSON action protocol

The model must answer with one JSON object.

Tool call:

```json
{"action":"tool","tool":"read_file","input":{"path":"package.json"}}
```

Range read:

```json
{"action":"tool","tool":"read_file","input":{"path":"src/tools.js","range":"10-40"}}
```

Glob call:

```json
{"action":"tool","tool":"glob_paths","input":{"patterns":["src/**/*.js","scripts/*.mjs"],"limit":200}}
```

Regex grep call:

```json
{"action":"tool","tool":"grep_regex","input":{"expr":"createToolRegistry","path":"src","caseSensitive":true}}
```

Command call, only when enabled:

```json
{"action":"tool","tool":"run_command","input":{"command":"npm run check","timeoutMs":30000}}
```

Package script call:

```json
{"action":"tool","tool":"run_package_script","input":{"script":"check","timeoutMs":60000}}
```



Todo call:

```json
{"action":"tool","tool":"todo","input":{"op":"init","items":["Inspect files","Apply fix","Verify behavior"]}}
```
Git diff call:

```json
{"action":"tool","tool":"git_diff","input":{"path":"src/tools.js"}}
```


Artifact call:

```json
{"action":"tool","tool":"save_artifact","input":{"name":"notes.txt","content":"research notes"}}
```

URL fetch call:

```json
{"action":"tool","tool":"fetch_url","input":{"url":"https://example.com","maxBytes":200000}}
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
