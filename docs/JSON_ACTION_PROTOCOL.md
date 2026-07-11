# JSON action protocol

Jeden accepts one JSON action object per model turn. Native model tool calls are normalized into the same action loop.

## Final answer

```json
{"action":"final","text":"Done."}
```

A final action ends the turn and records the answer in the session transcript.

## One tool

```json
{"action":"tool","tool":"read_file","input":{"path":"package.json"}}
```

The runtime validates the tool name and input schema, applies hooks and approval policy, executes the tool, and feeds the result into the next model turn.

## Multiple tools

```json
{"action":"tools","tools":[{"tool":"read_file","input":{"path":"package.json"}},{"tool":"git_status","input":{}}]}
```

Read-only calls may run concurrently when hooks and approvals are inactive. Writes, commands, hook-managed calls, and approval-gated calls stay serialized.

## Read selectors

A `read_file` path may carry a line range, raw mode, or conflict selector. The response includes the content digest, snapshot tag, and a numbered visual snapshot. Mutations must use the current digest or snapshot tag.

Document, archive, image, SQLite, artifact, URL, and MCP reads use their dedicated tools and return structured JSON results.

## Processes and evaluation

`run_command` executes a shell command when command permission is enabled. `run_process` executes an argv array without a shell. `run_package_script` executes a declared package script. `node_eval` and `python_eval` run code through their dedicated runtimes. All remain subject to the active approval and hook policy.

## Artifacts, memory, and delegation

- `save_artifact`, `list_artifacts`, and `read_artifact` manage session artifacts.
- `memory` stores or recalls scoped durable notes.
- `todo` manages the active phased task list.
- `delegate_task` runs a focused child Jeden session and returns its status, answer, and session path.

## MCP

The generic MCP actions are `mcp_list_tools`, `mcp_call_tool`, `mcp_list_resources`, `mcp_read_resource`, `mcp_list_prompts`, and `mcp_get_prompt`. Configured server tools may also appear as native `mcp__<server>__<tool>` calls.

## Anchored visual patches

The native `edit` action accepts a patch string anchored by the snapshot returned from `read_file`:

```text
*** Begin Patch
[src/file.js#TAG]
SWAP N.=N:
+const ok = true
INS.POST N:
+export { ok }
*** End Patch
```

Supported operations are:

- `SWAP N.=M:` and `SWAP.BLK N:`
- `DEL N`, `DEL N.=M`, and `DEL.BLK N`
- `INS.PRE N:`, `INS.POST N:`, `INS.HEAD:`, `INS.TAIL:`, and `INS.BLK.POST N:`
- `REM`
- `MV path`

`SWAP` and `INS` bodies use `+` lines. Deletion uses `DEL`, `DEL.BLK`, or `REM`. Block operations resolve safe Markdown-heading, brace-block, or indentation-block boundaries. Unknown anchors are rejected. The `[path#TAG]` value must match the current snapshot.

Mutation results retain the JSON API and include a visual diff preview.

Tool results are fed back into the next model turn. The loop stops on `final` or when the configured step boundary is reached.
