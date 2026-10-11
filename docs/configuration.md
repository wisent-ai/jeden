# Configuration and context

Where Jeden reads its configuration from, and what it puts in front of a model
before a run. Moved out of the README because a reader who wants to install the
product should not have to scroll through every lookup path to reach the next
instruction.

User config loads from `~/.jeden/config.json` and `~/.jeden/config.yml`. Project config loads from `<cwd>/.jeden/config.json` and overrides user config. Environment variables still win over file config.

A configuration layer that exists and cannot be read, is not JSON (`.json`) or YAML (`config.yml`), has no object at its root, or holds a value its setting cannot take is never read as empty: every `jeden` command refuses to start with `Error: configuration <path> cannot be read: <cause>`, `… is not valid: <cause>` or `… does not hold valid settings: <cause>` and a failure status. A layer that breaks while a run is under way stops that run with the same sentence. A file that does not exist is simply no layer.

Before each run, Jeden loads user context from `~/.jeden/instructions.md` and `~/.jeden/context.md`. Project context walks from the project ancestor to `--cwd` and reads:

- `JEDEN.md`
- `AGENTS.md`
- `CLAUDE.md`
- `RULES.md`
- `.jeden/instructions.md`
- `.jeden/context.md`

A context line such as `@./extra.md` imports another file under the same context root. Oversized context files are skipped.

File-based custom commands load from project and user `.jeden/commands/` directories. Native extensions load from project and user `.jeden/extensions/` directories. Plugin and marketplace state lives under `~/.jeden/plugins/`.

`jeden rpc` publishes executable file-based commands as `quickReplies` in both
the `ready` frame and the `capabilities` response. Each entry carries its
capability ID, label, slash prompt, and discovery source; native clients use
that projection instead of reproducing command-directory precedence.

### The context advisor

The files above are what is always true here. What is relevant to the request
that just arrived is a different question, and `jeden context` answers it with
locators rather than prose: `path:first-last` for a chunk of a file,
`memory:<id>` for a recalled memory, `session:<id>` for a transcript, and
`repo/path@commit:first-last` for a ground-truth citation.

Four sources answer, each owned where it belongs. `files` reads everything
readable under the declared roots — documentation, source code, configuration,
manifests — cutting Markdown at its headings and every other file into
forty-line windows titled by the declaration they open with. `memory` recalls
what earlier sessions in this workspace wrote down. `transcripts` runs
Transcript Lake's own search over the masked archive. `ground-truth` asks the
Wisent cross-repository index for cited chunks. Every answer reports each
source's state — available or unavailable, with the observed reason — so a
short list is never mistaken for a complete one.

Before its first model call, every turn receives the top recommendations as a
`[Context recommendations]` block, recorded in the session's own `user` event.
`context.advisor.sources` defaults to `files,memory`, the two that answer from
local state; `--source all` adds the archive and the index. Sources run
concurrently and nothing cuts one short: there is no deadline setting and no
`--timeout-ms` flag, because a guessed interval reports nothing and explains
nothing. Choose the source set explicitly. `jeden context prompt "<task>"` prints exactly what the next turn would
receive, `jeden context sources` reports what each source is and whether it
answers now, and `context.advisor.enabled false` switches the block off.
The full contract is at
[jeden.wisent.com/docs/context](https://jeden.wisent.com/docs/context).


### Automatic compaction and tool-result pruning

A conversation compacts on its own only at a token count the operator states.
`JEDEN_COMPACTION_THRESHOLD` names that count directly; otherwise
`JEDEN_CONTEXT_LIMIT` less `JEDEN_COMPACTION_RESERVE` does. With neither,
compaction is `/compact` alone. A context limit stated without its reserve is
refused by name (`JEDEN_CONTEXT_LIMIT is <n> but JEDEN_COMPACTION_RESERVE is not
set`), and so is a reserve that leaves nothing of the limit: Jeden does not pick
a share of the window for you.

Before compacting, Jeden can replace old tool results with a one-line marker.
That step runs only when all three of its bounds are stated:
`JEDEN_TOOL_PRUNE_PROTECT_TOKENS` (the newest tool results kept whole),
`JEDEN_TOOL_PRUNE_MIN_SAVINGS_TOKENS` (the least a prune must free) and
`JEDEN_TOOL_PRUNE_MIN_TOOL_TOKENS` (the smallest result worth replacing). With
none stated the step is skipped; with only some, the turn is refused with the
names of the missing ones.

### Never-ending chat memory

With `chatMemory` in `.jeden/config.json` (project) or `~/.jeden/config.json`
(user), a session keeps every message and stops compacting:

```json
{ "chatMemory": { "summaryModel": "<cheap Brama route or alias>", "viewBytes": <bytes a turn's view may take> } }
```

- Every message the conversation holds — prompts, replies, tool calls and the
  tool results the model received — is appended word for word to
  `<session>/chat-memory/log.jsonl` and never rewritten, with every field it
  carries (an assistant turn's `tool_calls`, a tool result's `tool_call_id`),
  so the message a turn is given back is the one the provider accepted.
- Over the log grows a tree of summaries in `chat-memory/tree.jsonl`: a leaf
  per message, a parent per two adjacent nodes of equal span, each at most the
  operator's summary size (declared in `rust/chat_memory/numeric-provenance.json`).
  A message or pair that already fits is kept word for word (`by: verbatim`;
  a pair whose halves include a model's summary is `joined`); anything longer is
  summarized by `summaryModel` through Brama, on the session's lease. Leaves the
  next fold needs are written on a background thread between steps.
- Each step sends the conversation's system messages, the view's nodes as one
  memory message (`[first-last] from..to` and the summary, the bytes the view
  counts), and the newest messages word for word — never more than
  `viewBytes`. When the view outgrows it, every tail message before the newest
  turn folds into its leaf at the end of the row — the newest message that is
  not a tool result stays with the tool results after it, since a provider
  refuses a tool result whose call is not just before it — then every adjacent
  pair of the finest span merges at once, so the beginning a provider caches
  changes only at those moments. The view is kept in `chat-memory/view.json`;
  a resumed session continues from it.
- `recall_conversation` with `node=<id>` zooms into a node (its two halves, or
  a message word for word); with `from` and `to` (seconds since 1970) it
  answers the messages of that time. `jeden chat-memory show|zoom|dates`
  answers the same from the command line, and Jeden Desktop's Chat memory
  screen shows the memory of the selected session, zooms into its nodes and
  lists the messages of a picked time.
- A `chatMemory` without `summaryModel` or a positive `viewBytes`, a
  `viewBytes` smaller than one summary, a summary model Brama's catalog
  refuses, and a view whose newest turn alone (the newest message, or an
  assistant turn with its tool results) outgrows `viewBytes` stop the
  turn with that cause. An unreadable log, tree or view is refused naming the
  file, never restarted.
