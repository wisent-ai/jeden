const sources =
  "<code>--source</code> takes any comma-separated subset of <code>files</code>, <code>ground-truth</code>, <code>memory</code>, <code>transcripts</code>, or <code>all</code>. Without it, <code>context.advisor.sources</code> decides, which defaults to <code>files,memory</code>.";
const bounds =
  "<code>--limit</code> caps the recommendations returned. Nothing cuts a source short: every source runs to completion. <code>--cwd</code> selects the workspace whose project configuration and memory scope apply.";
const target =
  "<code>--omp</code> resolves <code>~/.omp/agent/tools/jeden_context.ts</code>, Omp's own documented custom-tool directory. <code>--file &lt;path&gt;</code> addresses any other location. One of the two is required.";
const unknownSource =
  "An unknown source is refused as <code>unknown source(s): &lt;name&gt;. Known sources: files, ground-truth, memory, transcripts</code>; a typo never narrows the answer silently.";

export const contextCommands = [
  {
    path: "context",
    invocation:
      'jeden context [recommend] "<task>" [--limit n] [--source list] [--json] [--cwd path]',
    purpose: "Ask the context advisor what to read for a task instead of searching for it.",
    inputs: [
      "A bare first word is taken as the task, so the verb may be omitted: <code>jeden context \"why does signing fail\"</code> is <code>jeden context recommend</code>.",
      sources,
      bounds,
    ],
    effect:
      "Prints ranked recommendations and then the state of every source consulted. It reads only: no session is created, no memory is written, and no configuration changes. The verbs are <code>recommend</code>, <code>prompt</code>, <code>sources</code>, <code>install</code> and <code>installed</code>.",
    refusals: [
      unknownSource,
      "A missing task is refused as <code>context recommend requires a task</code> followed by the usage block.",
      "An unrecognised flag is refused as <code>unknown context option: &lt;flag&gt;</code> with the usage block.",
    ],
  },
  {
    path: "context/recommend",
    invocation:
      'jeden context recommend "<task>" [--limit n] [--source list] [--json] [--cwd path]',
    purpose: "Return ranked locators for a task, with the state of every source that answered.",
    inputs: [sources, bounds, "<code>--json</code> returns the authoritative object."],
    effect:
      "Each recommendation carries <code>source</code>, <code>title</code>, <code>locator</code>, <code>score</code>, <code>matched</code> and <code>snippet</code>. Locators are <code>path:first-last</code> for a file chunk, <code>memory:&lt;id&gt;</code> for a recalled memory, <code>session:&lt;id&gt;</code> for a transcript, and <code>repo/path@commit:first-last</code> for a ground-truth citation. The <code>sources</code> array carries <code>available</code>, the observed <code>detail</code>, <code>considered</code>, <code>returned</code> and <code>elapsedMs</code> for each source, so a short list can be told apart from a refused one.",
    refusals: [
      unknownSource,
      "A missing task is refused as <code>context recommend requires a task</code>.",
      "A resolved source list that is empty is refused as <code>no source selected: context.advisor.sources resolved to nothing</code>.",
      "A source that cannot answer never fails the command: it is reported as <code>unavailable</code> with its reason, such as <code>no endpoint: set context.advisor.groundTruthUrl or WISENT_GROUND_TRUTH_API</code> or <code>exited with exit status: 1</code>.",
    ],
  },
  {
    path: "context/prompt",
    invocation: 'jeden context prompt "<task>" [--json] [--cwd path]',
    purpose: "Print exactly the recommendation block a turn would append for this task.",
    inputs: [
      "The task text. The resolved <code>context.advisor</code> settings apply; per-run source and limit flags belong to <code>recommend</code>.",
      "<code>--json</code> returns <code>query</code>, <code>enabled</code>, <code>sources</code>, <code>maxChars</code> and <code>block</code>, where <code>block</code> is <code>null</code> when nothing would be appended.",
    ],
    effect:
      "Renders the <code>[Context recommendations]</code> block, bounded by <code>context.advisor.maxChars</code>, including the list of sources that answered nothing. It calls no model and records no session; it is the way to see what the next turn will actually receive.",
    refusals: [
      "A missing task is refused as <code>context prompt requires a task</code>.",
      "With the advisor switched off the command succeeds and says why: <code>The context advisor is off: context.advisor.enabled is false.</code>",
      "With no source selected it succeeds and says <code>No context source is selected: context.advisor.sources resolved to nothing.</code>",
    ],
  },
  {
    path: "context/sources",
    invocation: "jeden context sources [--json] [--cwd path]",
    purpose: "Report what each context source is configured to be and whether it answers now.",
    inputs: [
      "No arguments. <code>--cwd</code> selects the workspace whose configuration and memory scope are reported.",
      "<code>--json</code> returns <code>settings</code> (every resolved advisor setting), <code>selected</code> (the sources every run consults) and <code>sources</code> (one probe per source).",
    ],
    effect:
      "Each probe carries the shared <code>available</code>, <code>detail</code>, <code>considered</code>, <code>returned</code> and <code>elapsedMs</code> fields plus what only that source has: the walked roots, their declared extensions, which roots exist or are missing, the file count and whether the walk was cut short; the memory store path; the Transcript Lake command; and the ground-truth endpoint with the <code>origin</code> that supplied it — the configuration key, an environment variable name, or <code>unset</code>.",
    refusals: [
      "Extra arguments return the usage block.",
      "An unconfigured ground-truth endpoint is reported, not an error: <code>no endpoint: set context.advisor.groundTruthUrl or WISENT_GROUND_TRUTH_API</code>. A configured endpoint nobody serves reports the URL it could not reach.",
    ],
  },
  {
    path: "context/install",
    invocation: "jeden context install [--omp|--file <path>] [--json]",
    purpose: "Install the advisor into another harness as a custom tool.",
    inputs: [
      target,
      "<code>--json</code> returns <code>target</code>, <code>path</code>, <code>tool</code> and <code>changed</code>.",
    ],
    effect:
      "Writes a <code>context_recommend</code> tool that calls <code>jeden context recommend --json</code> through the absolute path of the binary that rendered it, so an Omp session reaches the same advisor a Jeden session does. The write is atomic and idempotent: an already current file reports <code>changed: false</code> and is not rewritten. No Omp source is modified.",
    refusals: [
      "Without a target: <code>context install and context installed require --omp or --file &lt;path&gt;</code>.",
      "<code>--file</code> without a value is refused as <code>--file requires a path</code>; an unknown flag as <code>unknown context option: &lt;flag&gt;</code>.",
      "<code>--omp</code> without <code>HOME</code> is refused as <code>HOME is not set</code> rather than guessing a directory.",
    ],
  },
  {
    path: "context/installed",
    invocation: "jeden context installed [--omp|--file <path>] [--json]",
    purpose: "Say whether the installed tool is exactly what this binary renders.",
    inputs: [target, "<code>--json</code> returns <code>state</code> beside the target and path."],
    effect:
      "Compares the file with the rendered tool and reports <code>current</code>, <code>stale</code> or <code>absent</code>. Only <code>current</code> exits zero; the other two exit non-zero and name the repair, so an upgrade that changed the tool is visible instead of silent. <code>status</code> is accepted as an alias.",
    refusals: [
      "Without a target: <code>context install and context installed require --omp or --file &lt;path&gt;</code>.",
      "<code>stale: &lt;path&gt; carries a different context_recommend tool; run jeden context install --&lt;target&gt;</code> and <code>absent: &lt;path&gt; carries no context_recommend tool; run jeden context install --&lt;target&gt;</code> are refusals, not warnings.",
    ],
  },
];
