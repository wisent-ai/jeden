export const sessionPages = [
  {
    slug: "sessions",
    href: "/docs/sessions",
    file: "sessions.html",
    meta: {
      htmlTitle: "Sessions — Jeden documentation",
      description:
        "Jeden sessions — durable, append-only session ledgers, checkpoints and rewind, durable memory, and configuration and context loading.",
      ogTitle: "Sessions — Jeden documentation",
      ogDescription: "Durable sessions, checkpoints, memory, and context in the Jeden harness.",
      canonical: "https://jeden.wisent.com/docs/sessions",
    },
    eyebrow: "Durable state",
    title: "Leave. Return. <em>Keep going.</em>",
    description:
      "Jeden records the work behind the answer — tool results, artifacts, decisions, and state — so a serious task can outlive a single interaction. Sessions are durable, append-only, and owned by the operator.",
    sections: [
      {
        title: "Where sessions live",
        paragraphs: [
          "Sessions live under <code>~/.jeden/sessions/</code> (<code>JEDEN_SESSION_ROOT</code> overrides). Each session directory holds <code>state.json</code> and <code>transcript.jsonl</code>, an append-only ledger of sequenced, parent-linked, checksum-sealed events that is validated on read and <code>fsync</code>ed on every append.",
          "Per-project state lives in <code><cwd>/.jeden/</code>. All of it is on the operator’s disk; Jeden uploads none of it, and session transcripts are never expired or deleted by Jeden. Backing up <code>~/.jeden/</code> and <code><cwd>/.jeden/</code> is the operator’s responsibility — Jeden ships no backup or restore command.",
          "Configured and automatically discovered secret values are replaced with <code>[REDACTED]</code> in the model-bound copy of the context, while the local transcript keeps the original text.",
          "Jeden emits no telemetry to Wisent from a default local run; the per-session event ledger is Jeden’s audit record. Every completion appends tokens, the Brama-catalog-priced cost breakdown, and the served billing target and decision ID to <code><cwd>/.jeden/usage.json</code>; <code>jeden stats</code> and <code>/usage show</code> read the ledger.",
        ],
      },
      {
        title: "Retained work and verified completion",
        paragraphs: [
          "The session's <code>completion.json</code> owns original requests, their workspace, acceptance requirements, task states and verification references. Atomic replacement and a process lock preserve them across interruptions. A model's <code>todo done</code> is only a verification request. It cannot remove user-owned tasks, change their acceptance criteria or approve its own result.",
          "Every proposed final answer is checked against all unresolved original requests and every recorded criterion. Work criteria require successful observations from a fresh read-only verifier. Questions and reading requests do not acquire implementation requirements. An unmet but actionable requirement returns control to the execution loop instead of ending the conversation; <code>assistantMessage</code> carries progress or an interim answer without claiming completion.",
          "Each inspection stage gets exactly one correction. When an intake or acceptance-review answer cannot be read as the required JSON object, or the native controller refuses the plan it carries, the exact refusal is recorded as <code>completion_rejected</code> and quoted back to a fresh read-only inspection. A second refusal records the durable blocker; one unusable answer no longer strands a retained request. A missing field is read the strict way rather than refused: an intake task without <code>kind</code> is work, and a review without <code>criteria</code> has reviewed no criterion, so the controller still refuses the acceptance and the loop keeps the work open.",
          "Model or verification failure records the actual operation and error and keeps <code>complete: false</code>. An explicit step limit reports <code>paused</code>; interruption reports <code>interrupted</code>. Operator pauses and cancellations remain authoritative. A resumed session inspects prior results before proposing repeated effects. The model still interprets whether observations satisfy natural-language criteria; the native controller checks coverage, evidence identity and state transitions rather than claiming infallible judgement.",
          "The place a criterion names is decided by the controller, not by that interpretation. When a work criterion names a path, an accepted verdict requires an accepted observation at that path, and a verdict resting on work done elsewhere is refused with <code>task &lt;id&gt; criterion &lt;n&gt; names &lt;path&gt;, and no accepted observation happened there</code>. A bare file name is read as the workspace root the request recorded, so a write one directory below it no longer satisfies the criterion; a path of several parts also matches an observation whose own path ends with those parts, because one file is spelled from different roots inside a receipt. Version numbers, host names and gateway addresses are not read as paths, and a criterion that names no path is left entirely to the reviewer's prose.",
          "Use the Conversation screen's <strong>Tasks</strong> panel in Desktop or mobile to read requests, acceptance criteria, blocker details and evidence references, or to pause, resume and cancel a task with a reason. <strong>Continue retained work</strong> uses the same session operation as the CLI and does not add an artificial user obligation. Reopening a graphical session reloads its current completion state; a terminal response event alone does not mean all work is complete.",
        ],
        commands: [{
          label: "Inspect and continue the retained session",
          code: 'jeden todo list --session <session> --json\njeden todo pause <request-id> --session <session> --revision <revision> --reason "Pause this request"\njeden todo list --session <session> --json\njeden todo resume <request-id> --session <session> --revision <new-revision> --reason "Resume this request"\njeden todo continue --session <session> --allow-write',
        }],
      },
      {
        title: "Inspect, export, resume",
        paragraphs: [
          "<code>jeden export</code>, <code>show</code>, <code>artifacts</code>, <code>artifact</code>, <code>search-sessions</code>, <code>resume</code>, and <code>recall_conversation</code> inspect or reuse recorded work. Resume inherits both selected history and retained tasks into a child session. Without a new prompt it continues unfinished work, preserving the recorded workspace and requiring explicit execution grants.",
        ],
        commands: [
          {
            label: "Session commands",
            code: 'jeden sessions\njeden show <session>\njeden export <session> <output>\njeden artifacts <session>\njeden artifact <session> <name> <output>\njeden resume <session> "continue"\njeden search-sessions "query"\njeden recall_conversation --list',
          },
        ],
      },
      {
        title: "Adopt existing work",
        paragraphs: [
          "<code>jeden workspace discover [path]</code> validates an existing directory without writing. <code>jeden workspace adopt &lt;path&gt;</code> then stores only its canonical absolute path as <code>workspace.defaultPath</code> in <code>~/.jeden/config.yml</code>, after validation succeeds. Relative input is resolved from the invocation directory; a path containing <code>..</code>, a missing or unreadable directory, or malformed existing <code>&lt;workspace&gt;/.jeden/config.json</code> is refused before user state changes.",
          "Adoption does not import or copy a repository, configuration, credential, or transcript. The working tree remains byte-for-byte where the user owns it. Session ledgers remain under the canonical session root and are associated by the <code>cwd</code> already recorded in each <code>state.json</code>. Re-adopting the same canonical path reports <code>unchanged</code>; unreadable session state is counted as rejected instead of silently omitted.",
          "The terminal first-use screen and <code>/setup workspace &lt;path&gt;</code> call the same operation as Jeden Desktop’s first-use folder chooser and Settings → Default workspace over <code>workspace/adopt</code> RPC. Both show the accepted canonical path and accepted/rejected session counts. Replay is <code>/onboarding reset</code> in the terminal and <strong>Settings → First-run walkthrough → Show it again</strong> on macOS.",
          "Jeden iOS cannot read or adopt a path from the phone. Its supported adapter is the canonical headless session protocol: after a host identity is configured, choose a row under <strong>On this host</strong>. Completion is recorded only after <code>session/open</code> resumes that exact ledger and <code>session/history</code> returns its retained turns. The next mobile prompt therefore uses the session’s recorded host workspace. The Settings gear shows every accepted host session and <strong>Choose an existing workspace session</strong> replays the same first-use selector. The host repository and ledger never leave the host.",
        ],
        commands: [
          {
            label: "Inspect and adopt without copying data",
            code: "jeden workspace discover /path/to/repository\njeden workspace adopt /path/to/repository\njeden workspace status",
          },
        ],
        callout: {
          tone: "note",
          text: "Workspace input is optional. Skipping first use keeps the current directory usable for that run and persists nothing. <code>--cwd &lt;path&gt;</code> always overrides an adopted default. On iOS, dismissing source selection changes neither the phone’s saved host records nor the daemon’s repository or sessions.",
        },
      },
      {
        title: "Checkpoints and rewind",
        paragraphs: [
          "<code>/checkpoint [label]</code> records the exact model-visible context, <code>/checkpoint list</code> prints durable checkpoint event IDs, and <code>/rewind <checkpoint-event-id></code> appends a new active lineage without deleting abandoned history.",
          "A session transcript with a truncated tail is read up to the last valid event, refuses further appends, and must be continued in a child session. In the interactive attachment tray, <code>/attach <relative-path></code>, <code>/attachments</code>, and <code>/detach <id|all></code> manage bounded, workspace-jailed text and PNG, JPEG, GIF, or WebP inputs consumed by the next submitted turn.",
        ],
      },
      {
        title: "Durable memory",
        paragraphs: [
          "Durable memory uses SQLite/FTS at <code>~/.jeden/memory.sqlite3</code> by default. <code>JEDEN_MEMORY_DB</code> selects another database; legacy <code>JEDEN_MEMORY_FILE</code> remains an input-path override.",
          "<code>/memory enqueue</code>, <code>/memory queue</code>, <code>/memory queue run</code>, <code>/memory queue drain</code>, and <code>/memory rebuild</code> expose durable worker and index maintenance; <code>/memory rebuild</code> reconstructs the memory index.",
        ],
      },
      {
        title: "Configuration and context",
        paragraphs: [
          "Process environment wins over every file. User config loads from <code>~/.jeden/config.json</code> and <code>~/.jeden/config.yml</code>; project config loads from <code><cwd>/.jeden/config.json</code> and overrides user config.",
          "Before each run, Jeden loads user context from <code>~/.jeden/instructions.md</code> and <code>~/.jeden/context.md</code>. Project context walks from the project ancestor to <code>--cwd</code> and reads:",
        ],
        bullets: [
          "<code>JEDEN.md</code>",
          "<code>AGENTS.md</code>",
          "<code>CLAUDE.md</code>",
          "<code>RULES.md</code>",
          "<code>.jeden/instructions.md</code>",
          "<code>.jeden/context.md</code>",
        ],
        callout: {
          tone: "note",
          text: "A context line such as <code>@./extra.md</code> imports another file under the same context root. Oversized context files are skipped. File-based custom commands load from project and user <code>.jeden/commands/</code> directories; native extensions load from <code>.jeden/extensions/</code>, and plugin and marketplace state lives under <code>~/.jeden/plugins/</code>.",
        },
      },
      {
        title: "Communication and functionality contracts",
        paragraphs: [
          "The <code>contracts.communication</code> setting tells Jeden how to write to you. The <code>contracts.functionality</code> setting tells it how to carry out work and what must be complete before it answers.",
          "The built-in communication contract asks for plain sentences under three headings: what was done, actual blockers, and decisions or actions that genuinely belong outside the agent. Work Jeden can perform stays in execution, not in a next-steps list. Your own text replaces the default; <code>none</code> disables that wording, not the native completion checks.",
          "Jeden adds the contracts to every new or rebuilt system prompt after its built-in engineering and task contracts. They supplement the built-in rules and cannot relax tool grants, path jails, safety checks, or evidence requirements. <code>jeden run /prompt</code> shows the contract in force, and <code>config/contracts/get</code> reports which one it is in <code>communicationSource</code> (<code>default</code>, <code>operator</code>, or <code>disabled</code>) with the default text in <code>communicationDefault</code>.",
          "The CLI and Jeden Desktop Settings screen edit the same user defaults in <code>~/.jeden/config.yml</code>; the Settings screen also shows the default text while it is in force. A project may override either key in <code><cwd>/.jeden/config.json</code>.",
        ],
        commands: [
          {
            label: "Set, inspect, replace, or turn off the contracts",
            code: 'jeden config get contracts.communication\njeden config set contracts.communication "Answer in Polish using three plain sentences."\njeden config set contracts.communication none\njeden config reset contracts.communication\njeden config set contracts.functionality "Finish the requested behavior before answering."\njeden config reset contracts.functionality',
          },
        ],
      },
      {
        title: "Task contract and delivery report",
        paragraphs: [
          "Every ordinary user turn, including a delegated task, carries Jeden’s built-in task contract. Completion means durable, reusable product functionality rather than a one-off action. Only an assigned implementation task authorizes product changes: a question, request to read or explain, or planning request does not. Defects related to the assigned task are repaired at their source; diagnostics must make failures actionable; and applicable CLI, GUI, and public documentation surfaces must agree.",
          "Behavioral tests live in the product’s <code>tests/&lt;area&gt;</code> tree and execute a complete lifecycle through the real product, its real interface, and real dependencies, observing the final state rather than accepting mocks, canned responses, dry runs, or syntax checks. Tests may be created and run directly with the product’s own tools; Probierz is optional. Every run retains its exact source revision, commands, exit statuses, supported reports, traces, screenshots and recordings, and actual result.",
          "<code>npm run test:contracts</code> compiles the real contract suite once, selects the exact executables from Cargo's artifact report, signs the native product, and runs the returned test executable directly. It never invokes Cargo again after signing. <code>target/contract-runs</code> retains source revision and changes, signed binary digests, command output and final state. Model access uses the configured Brama identity and remains a real dependency.",
          "The report explains <code>functionality</code>, <code>diagnostics</code>, <code>cli</code>, <code>gui</code>, <code>documentation</code>, <code>tests</code> and <code>delivery</code>. It is an execution agent's claim, not evidence of completion. The independent acceptance pass still checks the real result and every retained original request.",
          "A malformed report is returned for correction while the invocation has steps available. Exhaustion remains a failed invocation with retained work. CLI, TUI, SDK, RPC and headless share these checks; model-only turns and Pursuit's separately reviewed stages retain their own output contracts.",
          "An answer that never arrives usable is corrected the same way, under the same event: a cut-off or unreadable answer is quoted back to the model once inside the turn as a <code>contract_violation</code> with <code>rule</code> <code>model-answer</code>, then the turn ends if the second answer is unusable too. The refusal says what happened to the answer — <code>model answer stopped mid-JSON after 191 bytes: a JSON string is never closed; the answer was cut off by the output budget of 48 tokens</code> — instead of a JSON parser's column, and the retained request stays open for the next turn. Jeden Desktop shows the same event as an answer correction rather than a report correction.",
          "The RPC <code>config/contracts/get</code> and <code>config/contracts/set</code> results include <code>taskContract</code>: a localized, versioned built-in description containing <code>instructions</code> and <code>requirements</code> (<code>id</code>, <code>title</code>, and <code>description</code>). It is read-only, not a third editable operator contract, and Jeden Desktop shows it read-only beside the editable communication and functionality settings. <code>/prompt</code> shows the contract active for the current session.",
        ],
        commands: [
          {
            label: "Required final action shape",
            code: `{
  "action": "final",
  "text": "Concise answer.",
  "report": {
    "functionality": {"status": "done", "explanation": "Reusable behavior implemented.", "evidence": ["src/feature.rs"]},
    "diagnostics": {"status": "done", "explanation": "Failures identify the cause and recovery.", "evidence": ["src/diagnostics.rs"]},
    "cli": {"status": "done", "explanation": "The existing CLI flow exposes the behavior.", "evidence": ["src/cli.rs"]},
    "gui": {"status": "not_applicable", "explanation": "This change has no GUI surface.", "evidence": []},
    "documentation": {"status": "done", "explanation": "Public usage is documented.", "evidence": ["web/docs/pages.mjs"]},
    "tests": {"status": "done", "explanation": "The real lifecycle and final state ran through Probierz.", "evidence": ["Probierz run jeden/feature"]},
    "delivery": {"status": "blocked", "explanation": "Release signing credential is unavailable.", "evidence": []}
  }
}`,
          },
          {
            label: "Inspect the active and RPC-projected contract",
            code: `jeden run "Implement the requested reusable behavior"
/prompt
printf '%s\n' '{"id":1,"method":"config/contracts/get","params":{}}' | jeden rpc`,
          },
        ],
      },
      {
        title: "Communication modes",
        paragraphs: [
          "The <code>communication.mode</code> setting chooses what Jeden shows of its own work while it answers. <code>normal</code> shows tool names while Jeden works and then the answer with its code. <code>debug</code> also shows every tool call with its input, every tool result, and the model's reasoning when the route streams it. <code>quiet</code> shows only the answer.",
          "Four keys override one item each and default to <code>auto</code>, which follows the mode: <code>communication.toolCalls</code>, <code>communication.toolResults</code>, <code>communication.reasoning</code>, and <code>communication.code</code>. Setting a key to <code>show</code> or <code>hide</code> wins over the mode, so <code>debug</code> with <code>communication.toolResults hide</code> shows calls and reasoning but no results.",
          "Hiding code replaces every fenced code block in an answer with <code>[code hidden: N lines]</code> and tells the model to answer in prose; tools still write and run code. The setting is read at the start of every turn, so <code>/settings set communication.mode quiet</code> changes the next turn of a running session, and Jeden Desktop edits the same values on its Settings screen. The session transcript records every tool call, result, and answer regardless of the mode.",
        ],
        commands: [
          {
            label: "Choose a mode and override one item",
            code: "jeden config set communication.mode debug\njeden config set communication.toolResults hide\njeden config set communication.code hide\njeden config get communication.mode\njeden config reset communication.code",
          },
        ],
      },
    ],
  },
];
