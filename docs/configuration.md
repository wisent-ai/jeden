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

A native extension module exports its extension as `default`, `extension`,
`activate`, `tool` or `tools` (looked for in that order). What it declares is
refused, and the extension marked unhealthy with the cause, rather than
completed: a tool states its input at most once (`parameters`, `input` or
`schema`; none means it takes no input), a command its prompt at most once
(`prompt`, `template` or `body`), a capability its `kind` and `version` as
strings, a provider its `loginMethods` as a list of `device_code`, `paste` or
`api_key`. `exec(command, args, options)` takes `args` as a list of strings
(`[]` for none) and answers the command's whole stdout and stderr. The host
reads every setting from the variables Jeden starts it with and refuses to run
without them.

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
`context.advisor.sources` defaults to `files,memory`. Files read local state;
memory opens the explicitly configured `JEDEN_DATABASE_URL` without a Skarbiec
lookup and reports its configuration, connection or query refusal when it
cannot answer. `--source all` adds the archive and the index. Sources run side
by side and nothing cuts one short: no setting or flag bounds how long a
source may take, because a guessed interval reports nothing and explains
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
a share of the window for you. The interactive session refuses to start, and
`/context` refuses to answer, while `JEDEN_CONTEXT_LIMIT` is set to anything
but a whole number above zero, naming the value it holds.

The status line's route segment is the service tier a model call would carry,
chosen in the same order (`JEDEN_SERVICE_TIER`, then `MODEL_SERVICE_TIER`,
then the fast mode tier); with none stated it shows no route. Its branch and
`dirty` mark come from one `git status --porcelain --branch`, run when the
working directory or the conversation changes rather than on every keypress,
so a change made outside Jeden shows after the next turn. A git that cannot
run, or fails inside a work tree, puts its cause where the branch would be.

Before compacting, Jeden can replace old tool results with a one-line marker.
That step runs only when all three of its bounds are stated:
`JEDEN_TOOL_PRUNE_PROTECT_TOKENS` (the newest tool results kept whole),
`JEDEN_TOOL_PRUNE_MIN_SAVINGS_TOKENS` (the least a prune must free) and
`JEDEN_TOOL_PRUNE_MIN_TOOL_TOKENS` (the smallest result worth replacing). With
none stated the step is skipped; with only some, the turn is refused with the
names of the missing ones.

### Never-ending chat memory

With `chatMemory` in `.jeden/config.json` (project) or `~/.jeden/config.json`
(user), an agent keeps one chat that never ends and stops compacting, as the
operator's OptChat design lays it out:

```json
{ "chatMemory": { "summaryModel": "<model on the selected route>", "chat": "<optional chat name>" } }
```

- A chat outlives its sessions. Every session of an agent appends to the same
  chat, so a new session, `/reset`, a resume, `/fork` and `/branch` go on from
  everything the agent was told before instead of starting from nothing. A
  new session's chat is named, in this order, by `JEDEN_CHAT` (what a
  launcher sets for each agent it starts), by `chatMemory.chat`, and otherwise
  it is the workspace's own chat, `workspace-<directory>-<SHA-256 of its
  path>`. Named chats live beside the session root in `chats/<name>/`
  (`~/.jeden/chats/` by default, beside `JEDEN_SESSION_ROOT` when that is
  set). A delegated task keeps its own chat in its session directory, so its
  steps stay out of the chat that delegated it. A name is letters, digits,
  `-`, `_` and `.`, not starting with a dot; anything else is refused before
  any model call: ``chatMemory.chat names the chat `<name>`, which is not a
  chat name: …`` (or `JEDEN_CHAT names the chat …`).
- A session keeps the chat it started in. Each session records the chat it
  writes to and which of its messages the chat holds in
  `<session>/chat-memory.json`; a resumed session goes on from its own record,
  and a session made from another (`/reset`, a resume's child, `/fork`,
  `/branch`) from its parent's, so it stays in that chat and the history it
  carries is not logged twice. `JEDEN_CHAT` and `chatMemory.chat` name the
  chat a new session starts in, so a launcher can set `JEDEN_CHAT` for every
  start and a session it resumes stays in its own chat; a `JEDEN_CHAT` that
  is no chat name is still refused. A session imported from another harness
  (`jeden import`) has no chat yet: the first run that continues it starts
  one, and the whole conversation it carries comes into that chat. The import
  reports the transcript's title as `null` when the transcript states none,
  and refuses a transcript it cannot read, or whose message lacks a role or
  carries content that is neither text nor a list of parts, naming the file,
  the record's byte offset and what is missing.
- Only the agent's own conversation writes to its chat: the one that keeps
  the task's completion, or a `--model-only` run. An intake or acceptance
  inspection and a Pursuit stage are the harness's machinery and keep their
  messages out of it.
- One process writes a chat. A second one is refused with ``the chat `<name>`
  is written by another process now (it holds <chat>/owner.lock); one process
  writes a chat, so give this agent its own with JEDEN_CHAT=<name> or
  chatMemory.chat``.
- Every message is appended word for word to `<chat>/main/YYYY-MM-DD.jsonl`,
  one line `{i, kind, text, size, date}` each, flushed, never edited: `user`
  (the user's words), `jeden` (a reply), `tool` (a tool call: its name and JSON
  input), `echo` (a tool's output, clipped to its head and tail), `work` (the
  report of a task given to `delegate_task`: `[<agent> <job id>]` and the
  subagent's final output, clipped like a tool's; a delegation that failed
  before a subagent ran stays the tool's `echo`, and the task's own steps stay
  in its own chat), `note` (what the chat records itself: a rewind, or a
  message of an imported session).
- Over the log grows a binary tree in `<chat>/tree/YYYY-MM-DD.jsonl`, one line
  `{l, i, text, size}` per node: node(0, i) is message i in one line, node(l,
  i) merges node(l-1, 2i) and node(l-1, 2i+1), and it is named `id+n`, its
  first message and how many it covers. A source that fits the summary size is
  its own line with no model call (a short message word for word, two short
  lines joined by a newline); anything longer is compressed by `summaryModel`
  on the selected direct provider or Brama route, shown the size as a ruler
  and asked again when the line is too long, keeping the shortest. Each node
  is built once. Compactions run
  side by side, as many at a time as the operating system says this machine
  runs threads; `jeden chat-memory show` prints that number.
- Each turn sends the conversation's system messages, then the view — one
  `id+n|text` line per node covering every message of the chat before the
  turn, every earlier session included, inside `<chat>` tags, never a whole
  message — and then the turn itself from the user's message on. Each new
  message appends its line; the lines of new messages are built in rounds,
  each round shown the view before it. Once the view passes the high size,
  the parent of every sibling pair in it is built, and one batch merges the
  most due sibling pairs, due = (T - last) / 2^l with T the messages in the
  chat and last the pair's last message, oldest first among equals, until it
  is at most the low size; so the view is a sawtooth between the two and its
  beginning stays put between batches. The view is saved in
  `<chat>/view.json` as its `[l, i]` pairs, loaded at start and never rebuilt
  from the log. The sizes, the tool output clip and the retry count are the
  operator's numbers, declared with his words in
  `rust/chat_memory/numeric-provenance.json`.
- Between turns a background thread builds the line of every message the
  view does not cover yet and the parent of every sibling pair in the view.
- `recall_conversation` with `id` and `n` opens line `id+n` into the two lines
  it was made from, down to the message whole for a line of one message, and
  with `date=<id>` answers when message id was said. `jeden chat-memory`
  lists the chats and shows, zooms and dates any of them from the command
  line, by chat name or by a session that wrote to it, and imports an earlier
  session into a chat as notes, once per session; Jeden RPC answers the same
  as `chat-memory/list`, `chat-memory/show`, `chat-memory/zoom`,
  `chat-memory/date` and `chat-memory/import`.
- A `/rewind` keeps the log as it is and appends a `note` naming the messages
  taken back; what is said after it follows the note.
- A `chatMemory` without `summaryModel`, one still naming `viewBytes`, a
  `chat` that is not a name, and a summary model the selected provider's
  catalog refuses stop the turn with that cause. An unreadable log, tree, view,
  chat record or session record is refused naming the file, never restarted.
- The real journeys are `node tests/chat_memory/journey.mjs` (the sawtooth,
  zoom, date, a next process continuing the kept view, and rewind; with
  `JEDEN_TEST_MAX_STEPS` and `JEDEN_TEST_BATCHES`) and
  `node tests/chat_memory/chats.mjs` (a new session answering what an earlier
  one was told, the second writer's refusal, `list`, `show` by session,
  `import` and its refusals, and the name refusals), both with `JEDEN_BIN`,
  `JEDEN_TEST_SOURCE_REVISION`, `JEDEN_TEST_MODEL` and
  `JEDEN_TEST_SUMMARY_MODEL`; they write their reports under
  `.build/chat-memory/`.
