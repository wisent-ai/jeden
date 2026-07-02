# Jeden

Jeden is Wisent's private agent harness. It keeps Wisent's model routing, policy, terminal loop, and local tools under our control instead of inheriting a generic coding-agent policy stack.

## Design contract

Jeden separates four planes:

1. **Inference** — every model call goes through the Wisent model router using HMAC-signed OpenAI-compatible chat completions.
2. **Policy** — the harness prompt is short, local, and explicit: no unrequested tests, no unrequested docs, no silent substitution, no command execution unless enabled.
3. **Tools** — tools are a small allowlisted registry with path-jail enforcement. Writes and commands are gated by CLI flags.
4. **Run loop** — the model can emit strict JSON actions or a native OpenAI `tool_calls` response. Invalid JSON is a hard failure when text actions are used.

The router call sends OpenAI-compatible tool definitions with JSON schemas derived from each tool input contract; if the model returns native tool calls, Jeden maps them back into the same local action loop.

## Current scope

The private M1 version includes:

- `jeden` interactive terminal mode.
- `jeden run "task"` one-shot mode.
- Session logs and artifacts under `~/.jeden/sessions/<id>/`.
- Model calls through `MODEL_ROUTER_URL`, `WISENT_APP_AGENT_ID`, and `WISENT_APP_AGENT_AUTH_SECRET`.
- Default model `claude-code-subscription`; override per run with `--model <name>`, `JEDEN_MODEL`, or config.
- Filesystem read tools: `list_dir`, `read_file`, `read_binary_file`, `search_text`, `search_files`, `glob_paths`, `grep_regex`. Recursive search and glob discovery use `git ls-files --exclude-standard` when available, so ignored files are skipped. `search_files`, `glob_paths`, and `grep_regex` accept `limit` and `skip` for pagination.
- File write tools: `write_file`, `apply_patch`, `edit_file`, `delete_file`, `move_file`. Existing file mutations require the `sha256` returned by `read_file` and require `--allow-write`.
- Git read tools: `git_status`, `git_diff`, `git_log`, `git_show`.
- Eval tools: `node_eval`, `python_eval`. Both require command permission.
- Web read tool: `fetch_url`.
- Artifact tools: `save_artifact`, `list_artifacts`, `read_artifact` persist large or reusable UTF-8 outputs under the active session artifact directory.
- Custom JavaScript tools auto-load from `~/.jeden/tools/*.js|*.mjs` and `<cwd>/.jeden/tools/*.js|*.mjs`.
- Session todo tool: `todo` stores task state under the active session artifacts directory.
- Delegation tool: `delegate_task` runs a focused subtask in a fresh Jeden session and is gated as command execution.
- Durable memory tool: `memory` stores and recalls notes across sessions from `~/.jeden/memory.jsonl`.
- MCP tools: `mcp_list_tools`, `mcp_call_tool`, `mcp_list_resources`, `mcp_read_resource`, `mcp_list_prompts`, and `mcp_get_prompt` support configured stdio MCP servers.
- Session todo tool supports phased `list`, `phase`, and task operations `init`, `append`, `start`, `done`, `drop`, `rm`, and `view`; state is stored as a session artifact.
- Set `JEDEN_MEMORY_FILE` to override the memory file path for tests or isolated runs.
- Existing file writes and patches require the `sha256` returned by `read_file`.
- Project context auto-loads user context, ancestor context, and cwd context files before each run.
- Interactive mode asks before executing writes or commands unless the matching `--allow-*` flag is passed.
- Shared hooks are loaded from `~/.shared-hooks/run-hook.mjs` for `user_prompt_submit`, `pre_tool_use:*`, `post_tool_use:*`, and `stop`.
- `npm test` runs zero-dependency parity tests for protocol, file tools, context imports, and session artifacts; `npm run check` remains the syntax gate.
- Tool results larger than the context cap are written to the active session artifacts directory and replaced in the model loop with a compact preview plus artifact path.

## CLI

```sh
jeden artifacts <session-id-or-path>
jeden artifact <session-id-or-path> notes.txt
jeden export <session-id-or-path> session.json
jeden --cwd ../content-platform
jeden --cwd ../content-platform --allow-command
jeden resume <session-id-or-path> "continue with the previous context" --cwd ../content-platform
jeden sessions 20
jeden search-sessions "needle" 50
jeden show <session-id-or-path>
jeden tools --cwd ../content-platform
jeden doctor --cwd ../content-platform
jeden run "summarize src/lib/api/model-router-hmac.ts" --cwd ../content-platform --model claude-code-subscription
jeden run "create notes.txt with hello" --cwd /tmp/sandbox --allow-write
```

Required env for real model calls:

```sh
WISENT_APP_AGENT_AUTH_SECRET=...
MODEL_ROUTER_URL=https://model-router-1080673333190.us-central1.run.app
WISENT_APP_AGENT_ID=wisent-app
```

Before each run, Jeden appends context files to the system prompt when present. User context loads from `~/.jeden/instructions.md` and `~/.jeden/context.md`; project context walks from the home/project ancestor down to `--cwd` and reads `JEDEN.md`, `AGENTS.md`, `CLAUDE.md`, `RULES.md`, `.omp/AGENTS.md`, `.omp/RULES.md`, `.jeden/instructions.md`, and `.jeden/context.md`. A context line like `@./extra.md` imports another file under the same context root. Oversized context files are skipped.



Use `jeden export <session-id-or-path> [output.json]` to write a portable JSON session transcript. Use `jeden export <session-id-or-path> --html [output.html]` for a standalone HTML transcript.


Use `jeden search-sessions <query> [limit]` to find matching recent session transcripts.
Use `jeden artifacts <session-id-or-path>` and `jeden artifact <session-id-or-path> <name> [output]` to inspect or extract session artifacts.

Use `jeden resume <session-id-or-path> "task"` to seed a new run with the prior session transcript summary while recording a fresh session.

Config files load from `~/.jeden/config.json` and `<cwd>/.jeden/config.json`; project config overrides user config. Supported keys: `model`, `modelRouterUrl`, and `agentId`. Existing environment variables still win. Use `jeden config --cwd .` to print the merged config.
Use `jeden doctor --cwd .` to print a JSON diagnostics report covering merged config, model-router env presence, built-in tool count, and custom tool load errors.
MCP servers load from `~/.jeden/mcp.json` and `<cwd>/.jeden/mcp.json` using the standard `mcpServers` shape for stdio servers. Add server names to `disabledServers` to block them after config merge.
MCP calls reject pending requests when their timeout elapses and escalate from `SIGTERM` to `SIGKILL` if the child does not exit.

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

Multiple tool calls:

```json
{"action":"tools","tools":[{"tool":"read_file","input":{"path":"package.json"}},{"tool":"git_status","input":{}}]}
```

Range read:

```json
{"action":"tool","tool":"read_file","input":{"path":"src/tools.js:10+30"}}
```

Glob call:

```json
{"action":"tool","tool":"glob_paths","input":{"patterns":["src/**/*.js","scripts/*.mjs"],"limit":200}}
```

Regex grep call:

```json
{"action":"tool","tool":"grep_regex","input":{"expr":"createToolRegistry","path":"src","caseSensitive":true}}
```

List artifacts:

```json
{"action":"tool","tool":"list_artifacts","input":{}}
```

Read artifact:

```json
{"action":"tool","tool":"read_artifact","input":{"name":"notes.txt"}}
```

Command call, only when enabled:

```json
{"action":"tool","tool":"run_command","input":{"command":"npm run check","timeoutMs":30000}}
```

Package script call:

```json
{"action":"tool","tool":"run_package_script","input":{"script":"check","timeoutMs":60000}}
```

Node eval call:

```json
{"action":"tool","tool":"node_eval","input":{"code":"console.log(2 + 2)"}}
```

Python eval call:

```json
{"action":"tool","tool":"python_eval","input":{"code":"print(2 + 2)"}}
```


Todo call:

```json
{"action":"tool","tool":"todo","input":{"op":"init","items":["Inspect files","Apply fix","Verify behavior"]}}
```

Delegate call:

```json
{"action":"tool","tool":"delegate_task","input":{"task":"summarize src/tools.js","maxSteps":6}}
```


MCP list call:

```json
{"action":"tool","tool":"mcp_list_tools","input":{"server":"filesystem"}}
```

MCP tool call:

```json
{"action":"tool","tool":"mcp_call_tool","input":{"server":"filesystem","tool":"list_allowed_directories","args":{}}}
```

Memory call:

```json
{"action":"tool","tool":"memory","input":{"op":"remember","text":"Repo uses npm run check for syntax validation","tags":["jeden"]}}
```

MCP resource call:

```json
{"action":"tool","tool":"mcp_read_resource","input":{"server":"filesystem","uri":"file:///tmp/notes.txt"}}
```

MCP prompt call:

```json
{"action":"tool","tool":"mcp_get_prompt","input":{"server":"prompts","name":"review","args":{"topic":"diff"}}}
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

Line edit call:

```json
{"action":"tool","tool":"edit_file","input":{"path":"src/file.js","expectedSha256":"...","ops":[{"op":"replace","start":10,"end":12,"content":"const ok = true"}]}}
```
Final:

```json
{"action":"final","text":"Done."}
```

Tool results are fed back into the next model turn. The loop stops on `final` or when `--max-steps` is reached.
