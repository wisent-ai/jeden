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
- Default model `claude-code-subscription`; override per run with `--model <name>`, `JEDEN_MODEL`, or config. Use `--max-tokens <n>` to override the per-call output token cap.
- Filesystem read tools: `list_dir`, `read_file`, `read_binary_file`, `read_image`, `read_archive`, `read_document`, `read_sqlite`, `search_text`, `search_files`, `glob_paths`, `grep_regex`. `read_file` supports selectors like `:10-30`, `:raw:10+5`, and `:conflicts`; returns `sha256`, `snapshot` (`path#TAG`), `visual` (`[path#TAG]` plus numbered lines), and selected text so edits can be anchored to the visible snapshot. Large files require selectors so returned content stays under the cap. `read_image` returns base64 plus mime type, byte count, dimensions, truncation state, and SHA-256 for PNG, JPEG, GIF, and WebP images. `read_archive` lists entries from `.zip`, `.tar`, `.tar.gz`, and `.tgz`, and reads entries as UTF-8 text, binary base64, document text, or image metadata; text/document entry reads accept line ranges. `read_document` extracts readable text from text, HTML, JSON, CSV/TSV tables, XML/RSS/Atom feeds, basic PDFs, and notebooks, and can apply line ranges before the byte cap.
- File write tools: `write_file`, `apply_patch`, `edit`, `edit_file`, `delete_file`, `move_file`. Existing file mutations require the `sha256` or snapshot tag guard returned by `read_file`, require `--allow-write`, and return OMP-style visual diffs/previews for inspection.
- Git read tools: `git_status`, `git_diff`, `git_log`, `git_show`.
- Eval/process tools: `node_eval`, `python_eval`, `run_process`. They require command permission; `run_process` accepts argv without a shell. `run_command`, `run_process`, and `run_package_script` accept `env` overrides; `null` env values remove inherited variables for that child process. Process outputs are capped and report `stdoutTruncated`/`stderrTruncated` metadata. Timeouts send SIGTERM to the spawned process group, then SIGKILL after a short grace period.
- Web read tools: `fetch_url` for raw text and `fetch_readable_url` for simplified readable text. Both accept `maxBytes`, `timeoutMs`, and line ranges, and return status, content type, byte counts, truncation state, and SHA-256. `fetch_readable_url` normalizes HTML, pretty-prints JSON, converts CSV/TSV responses to compact Markdown tables, extracts RSS/Atom item titles and links, extracts basic PDF/notebook document text from URL responses, and can apply line ranges before the byte cap.
- Artifact tools: `save_artifact`, `list_artifacts`, `read_artifact` persist large or reusable UTF-8 outputs under the active session artifact directory.
- Custom JavaScript tools auto-load from `~/.jeden/tools/*.js|*.mjs` and `<cwd>/.jeden/tools/*.js|*.mjs`; their `exec` helper is gated by command permission.
- Session todo tool: `todo` stores task state under the active session artifacts directory.
- Delegation tool: `delegate_task` runs a focused subtask in a fresh Jeden session and is gated as command execution.
- Interactive ask tool: `ask_user` lets the model ask the human a question during `jeden` terminal mode.
- Automated durable memory: `memory` stores structured scoped MemoryRecord entries in `~/.jeden/memory.jsonl`; `runJeden` recalls relevant repo/global memories into the system prompt and records completed run episodes automatically after final answers. Memory failures are recorded as `memory_error` events and do not fail otherwise successful runs.
- MCP tools: `mcp_list_tools`, `mcp_call_tool`, `mcp_list_resources`, `mcp_read_resource`, `mcp_list_prompts`, and `mcp_get_prompt` support configured stdio MCP servers. Configured MCP server tools are also surfaced natively as `mcp__<server>__<tool>` with the server-provided input schema.
- Session todo tool supports phased `list`, `phase`, and task operations `init`, `append`, `start`, `done`, `drop`, `rm`, and `view`; state is stored as a session artifact.
- Set `JEDEN_MEMORY_FILE` to override the memory file path for tests or isolated runs. The core memory backend is local JSONL; optional semantic/graph backends such as Cognee can be plugged in behind the exported backend adapter interface without changing the runtime contract.
- Existing file writes and patches require the `sha256` or snapshot tag guard returned by `read_file`.
- Project context auto-loads user context, ancestor context, and cwd context files before each run.
- Interactive mode asks before executing writes or commands unless the matching `--allow-*` flag is passed.
- Shared hooks are loaded from `~/.shared-hooks/run-hook.mjs` for `user_prompt_submit`, `pre_tool_use:*`, `post_tool_use:*`, and `stop`.
- Tool permission and hook classification are centralized in `TOOL_CAPABILITIES`, so approval and hook routing share the same metadata.
- `npm test` runs zero-dependency parity tests for protocol, file tools, context imports, and session artifacts; `npm run check` remains the syntax gate.
- Tool results larger than the context cap are written to the active session artifacts directory and replaced in the model loop with a compact preview plus artifact path.

## Tool policy

- Use `glob_paths` or `list_dir` for file discovery before reading unknown paths.
- Use `grep_regex` or `search_files` for content search; do not use `run_command` or `run_process` for grep/find/ls/glob tasks covered by built-in tools.
- Use `read_file` selectors/ranges for targeted reads instead of dumping large files.
- Retry one alternate search pattern or narrower path before concluding a file or symbol is absent.
- Use `run_package_script` for package scripts after `list_package_scripts`; reserve `run_command` and `run_process` for commands without a safer built-in tool.
- Verify behavior changes with the narrowest relevant tool or package script before final output.

## CLI

```sh
jeden artifacts <session-id-or-path>
jeden artifact <session-id-or-path> notes.txt
jeden export <session-id-or-path> session.json
jeden export <session-id-or-path> --html session.html
jeden export <session-id-or-path> --markdown session.md
jeden --cwd ../content-platform
jeden --cwd ../content-platform --allow-command
jeden resume <session-id-or-path> "continue with the previous context" --cwd ../content-platform
jeden sessions 20
jeden search-sessions "needle" 50
jeden recall_conversation --list --cwd ../content-platform
jeden recall_conversation <session-uuid> --cwd ../content-platform
jeden show <session-id-or-path>
jeden tools --cwd ../content-platform
jeden capabilities --cwd ../content-platform
jeden doctor --cwd ../content-platform
jeden run "summarize src/lib/api/model-router-hmac.ts" --cwd ../content-platform --model claude-code-subscription
jeden run "create notes.txt with hello" --cwd /tmp/sandbox --allow-write
jeden run "fix the failing parser test" --cwd ../content-platform --self-repair --allow-write --allow-command
jeden run "summarize package.json" --json
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

Use `jeden resume <session-id-or-path> "task"` to seed a new run with structured replay of the prior session's user, assistant, and tool-result messages while recording a fresh session.
Use `jeden recall_conversation [session-uuid-or-filename] [--list] [--cwd path]` to reproduce `~/.claude/scratch/recall_conversation.sh` inside Jeden: it reads Claude session JSONLs from `~/.claude/projects/<encoded-cwd>/`, emits text-only `[USER]`/`[ASSISTANT]` blocks, and strips tool_use/tool_result/images/hooks/noise. `RECALL_CWD` still overrides the default project cwd; `--cwd` is available for explicit selection.
Use `--self-repair` on `jeden run` or `jeden resume` to start one bounded repair turn when the primary run fails. The failed session records a `run_error`, then the repair turn receives the original task, failure message, and transcript path. Without `--allow-write`, repair is diagnose/propose only; with `--allow-write`, it may apply a minimal project-scoped patch, and with `--allow-command` it may also run narrow verification. If `--cwd` is inside the Jeden package itself, writes/commands are stripped from the repair turn unless `--self-repair-own-code` is also passed.



Config files load from `~/.jeden/config.json` and `<cwd>/.jeden/config.json`; project config overrides user config. Supported keys: `model`, `modelRouterUrl`, and `agentId`. Existing environment variables still win. Use `jeden config --cwd .` to print the merged config.
Use `jeden doctor --cwd .` to print a JSON diagnostics report covering merged config, model-router env presence, built-in/custom/native MCP tool counts, and tool load errors.
Use `jeden capabilities --cwd .` to print the resolved capability manifest, including runtime defaults and the full built-in/custom/MCP tool surface.
MCP servers load from `~/.jeden/mcp.json` and `<cwd>/.jeden/mcp.json` using the standard `mcpServers` shape for stdio servers. Add server names to `disabledServers` to block them after config merge.
MCP clients stay open for the duration of a `jeden run`, so native MCP calls can reuse server state across steps; pending requests time out and close the client, with `SIGTERM` then `SIGKILL` cleanup. During `jeden run`, configured MCP tools are listed beside built-ins under native names like `mcp__filesystem__read_file`, so models can call them directly without first using `mcp_list_tools`.

Hook stdout may be empty, plain text, or JSON. JSON supports `decision: "block"` with `reason`, `userMessage` to replace the submitted prompt, and `toolInput` to replace the current pre-tool input. Jeden records hook results plus requested/effective tool payloads in the session transcript.

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

Custom tools may declare `permission: "write"` or `permission: "command"` plus optional `hook`/`postHook` metadata. Write permission defaults to edit hooks; command permission defaults to bash pre/post hooks. The `readText` helper is jailed to `cwd`; `exec` still requires command permission.

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

Read-only multi-tool calls run in parallel when hooks and approvals are not active; writes, commands, hooks, and approval-gated tools stay serialized.

Range read:

```json
{"action":"tool","tool":"read_file","input":{"path":"src/tools.js:10+30"}}
```

Additional read selectors:

```json
{"action":"tool","tool":"read_file","input":{"path":"src/tools.js:5-8,20-22"}}
{"action":"tool","tool":"read_file","input":{"path":"src/tools.js:raw:10+5"}}
{"action":"tool","tool":"read_file","input":{"path":"src/tools.js:conflicts"}}
```

Document read:

```json
{"action":"tool","tool":"read_document","input":{"path":"analysis.ipynb"}}
{"action":"tool","tool":"read_document","input":{"path":"paper.pdf","maxBytes":200000}}
```

Archive read:

```json
{"action":"tool","tool":"read_archive","input":{"path":"bundle.zip"}}
{"action":"tool","tool":"read_archive","input":{"path":"bundle.zip","entry":"src/index.js"}}
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

Argv process call without a shell:

```json
{"action":"tool","tool":"run_process","input":{"command":"node","args":["-e","console.log(process.argv[1])","hello"],"timeoutMs":30000}}
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

`delegate_task` returns child process status plus `delegated` parsed from the child `--json` run output: `{ ok, text, sessionPath }` when the child completed normally.


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
{"action":"tool","tool":"memory","input":{"op":"remember","kind":"project_fact","text":"Repo uses npm run check for syntax validation","tags":["jeden"],"confidence":0.8}}
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

Line edit call (`start`/`end`, `startLine`/`endLine`, or `line` are accepted for 1-based line anchors):

```json
{"action":"tool","tool":"edit_file","input":{"path":"src/file.js","expectedSha256":"...","ops":[{"op":"replace","start":10,"end":12,"content":"const ok = true"}]}}
```

OMP-style anchored visual patch call:

```json
{"action":"tool","tool":"edit","input":{"patch":"*** Begin Patch\n[src/file.js#A1B2]\nSWAP 10.=10:\n+const ok = true\nINS.POST 11:\n+export { ok }\n*** End Patch"}}
```

Supported visual patch hunks are `SWAP N.=M:`, `SWAP.BLK N:`, `DEL N` / `DEL N.=M`, `DEL.BLK N`, `INS.PRE N:`, `INS.POST N:`, `INS.HEAD:`, `INS.TAIL:`, `INS.BLK.POST N:`, `REM`, and `MV path`. `SWAP`/`INS` hunks require `+` body lines; use `DEL`/`DEL.BLK`/`REM` for deletion. Block hunks use safe markdown-heading, brace-block, or indentation-block boundaries and reject unrecognized anchors. The `[path#TAG]` tag must match the current `read_file.snapshot` tag.

`read_file` also returns an OMP-style `visual` snapshot alongside JSON metadata:

```txt
[src/file.js#A1B2]
10:const oldValue = false
11:export { oldValue }
```

Mutation tools keep the JSON API but now return a `diff` field with a visual hunk preview:

```txt
--- src/file.js
+++ src/file.js
@@ -10,2 +10,2 @@
-const oldValue = false
+const oldValue = true
 export { oldValue }
```
Final:

```json
{"action":"final","text":"Done."}
```

Tool results are fed back into the next model turn. The loop stops on `final` or when `--max-steps` is reached.
