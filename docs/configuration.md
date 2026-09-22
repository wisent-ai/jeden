# Configuration and context

Where Jeden reads its configuration from, and what it puts in front of a model
before a run. Moved out of the README because a reader who wants to install the
product should not have to scroll through every lookup path to reach the next
instruction.

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
nothing. Measured here, one archive search took 30 s against 1 s for the two
local sources, so choosing the source set is the decision that replaces that
guess. `jeden context prompt "<task>"` prints exactly what the next turn would
receive, `jeden context sources` reports what each source is and whether it
answers now, and `context.advisor.enabled false` switches the block off.

`jeden context install --omp` renders the same advisor into
`~/.omp/agent/tools/jeden_context.ts`, Omp's own documented custom-tool
directory, as the `context_recommend` tool bound to this binary; `jeden context
installed --omp` exits non-zero when that file is stale or absent. No Omp
source is modified. The full contract is at
[jeden.wisent.com/docs/context](https://jeden.wisent.com/docs/context).

