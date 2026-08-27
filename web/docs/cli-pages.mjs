const ORIGIN = "https://jeden.wisent.com";

function commandPage({ path, invocation, purpose, inputs, effect, refusals }) {
  const command = path.split("/").join(" ");
  const href = `/docs/cli/${path}`;
  return {
    slug: `cli-${path.replaceAll("/", "-")}`,
    navLabel: command,
    href,
    file: `cli/${path}.html`,
    meta: {
      htmlTitle: `${command} — Jeden CLI documentation`,
      description: `${invocation} — invocation, inputs, output, state effects, and refusal conditions.`,
      ogTitle: `${command} — Jeden CLI documentation`,
      ogDescription: purpose,
      canonical: `${ORIGIN}${href}`,
    },
    eyebrow: "CLI command",
    title: `<code>jeden ${command}</code>`,
    description: purpose,
    sections: [
      {
        title: "Exact invocation",
        commands: [{ label: "Shell", code: invocation }],
      },
      {
        title: "Inputs and options",
        bullets: inputs,
      },
      {
        title: "Output and state effect",
        paragraphs: [effect],
      },
      {
        title: "Refusals and boundaries",
        bullets: refusals,
      },
    ],
  };
}

const commands = [
  {
    path: "run",
    invocation: 'jeden run "task" [--json] [--model-only] [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]',
    purpose: "Run one concrete agent task through a durable Jeden conversation.",
    inputs: [
      "Required: a non-empty task after <code>run</code>.",
      "<code>--cwd</code>, <code>--model</code>, <code>--max-tokens</code>, and <code>--max-steps</code> select the workspace, route, response budget, and step bound.",
      "<code>--allow-write</code> and <code>--allow-command</code> grant those tool tiers; <code>--yolo</code>/<code>--auto-approve</code> grants both. <code>--model-only</code> suppresses tools and <code>--json</code> wraps local slash-command output where supported.",
    ],
    effect: "Creates or continues a local session ledger under the configured session root, calls the selected Brama model route, executes approved tools in the workspace, records usage, and prints the final answer.",
    refusals: [
      "A missing task is refused exactly as <code>run requires a task</code>.",
      "A missing value for a valued option is refused (for example <code>--model requires a value</code>); non-integer token or step bounds are refused.",
      "Model work fails closed when required router configuration or credentials are absent, and write or command tools remain approval-gated unless their tier was granted.",
    ],
  },
  {
    path: "pursue",
    invocation: 'jeden pursue "rough objective" [--json] [--cwd path] [--model name] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]',
    purpose: "Turn a rough objective into a source-grounded autonomous contract, execution, independent review, verdict, and receipt.",
    inputs: [
      "Required: a non-empty rough objective.",
      "The workspace, model route, grants, JSON output, and maximum steps use the same options as <code>run</code>.",
      "Transcript Lake preference evidence is consulted only when command execution is granted; otherwise pursuit proceeds without that external executable.",
    ],
    effect: "Runs Pursuit stages through persistent planner/executor conversations and fresh read-only reviewers, then prints or returns JSON containing the contract, verdict, receipt, and summary paths.",
    refusals: [
      "A missing objective is refused as <code>pursue requires a task</code> by the CLI parser; the command itself also rejects an empty value as <code>pursue requires a rough objective</code>.",
      "The run fails if Pursuit cannot establish its contract, execute it, or produce its verdict and durable receipt.",
      "Tool mutations remain subject to the same write and command grants as <code>run</code>.",
    ],
  },
  {
    path: "rpc",
    invocation: "jeden rpc",
    purpose: "Serve Jeden's newline-delimited JSON RPC interface on standard input and output.",
    inputs: [
      "No command-specific positional input or option is required.",
      "Clients send one JSON request per line with an <code>id</code>, <code>method</code>, and optional <code>params</code> object.",
    ],
    effect: "Keeps an in-process session map, emits one-line JSON responses and interaction events, and creates normal Jeden session state for requests that start or resume work. It opens no listener.",
    refusals: [
      "Frames larger than 1 MiB are refused.",
      "Malformed JSON, missing methods, unknown methods, invalid parameters, and writes to a closed output stream are returned as RPC errors rather than guessed.",
    ],
  },
  {
    path: "headless",
    invocation: "jeden headless <addr> <server-cert.pem> <server-key.pem> <client-ca.pem> <identity-map.json> [revoked-serials.txt]",
    purpose: "Serve the multi-tenant Jeden RPC service over a mutually authenticated TLS listener.",
    inputs: [
      "Required: bind address, server certificate chain, server private key, client CA bundle, and a JSON identity map.",
      "Optional: a text file of revoked client-certificate serials.",
      "Each identity-map entry must supply <code>san</code>, <code>principal</code>, and <code>tenant</code>.",
    ],
    effect: "Creates <code>.jeden/headless</code> service state, a durable reconnect key, tenant idempotency/replay stores, and an mTLS listener at the requested address.",
    refusals: [
      "Any argument count other than five or six is refused with the exact usage line shown above.",
      "Unreadable or invalid identity maps, empty maps, invalid mappings, TLS material failures, revoked certificates, certificates without an identity SAN, and bind failures are refused.",
      "Tenant request, session, and stored-byte limits are enforced instead of admitting excess work.",
    ],
  },
  {
    path: "acp",
    invocation: "jeden acp",
    purpose: "Serve Jeden as an Agent Client Protocol agent over standard input and output.",
    inputs: [
      "No command-specific positional input is required.",
      "The ACP client supplies session initialization, workspace and prompt requests on stdio.",
    ],
    effect: "Maps ACP sessions and content blocks onto Jeden SDK sessions, streams protocol events on stdout, and records ordinary local session state. It opens no network listener.",
    refusals: [
      "Invalid ACP messages, unsupported content, missing sessions, and workspace or model failures are returned as protocol errors.",
      "Filesystem and command effects still pass through Jeden's grants and approvals.",
    ],
  },
  {
    path: "collab-relay",
    invocation: "jeden collab-relay [addr]",
    purpose: "Run the encrypted collaboration-room relay used by interactive collaboration commands.",
    inputs: [
      "Optional: a listen address; the default is <code>127.0.0.1:8877</code>.",
      "Room payloads are opaque, client-encrypted blobs; mutation requests carry the room write token and role.",
    ],
    effect: "Binds an HTTP relay, keeps room blobs in the relay store, prints the bound address, and serves until stopped.",
    refusals: [
      "Bind failures stop startup.",
      "The relay refuses missing or invalid write tokens, empty bodies, blobs over 1 MiB, full rooms, unsupported methods, invalid roles, and unknown routes with explicit HTTP errors.",
    ],
  },
  {
    path: "sessions",
    invocation: "jeden sessions [limit]",
    purpose: "List locally stored Jeden session identifiers.",
    inputs: [
      "Optional: a positive integer-style positional limit. With no limit, every directory in the session root is considered.",
      "The session root is <code>~/.jeden/sessions</code> unless <code>JEDEN_SESSION_ROOT</code> overrides it.",
    ],
    effect: "Reads session directory names and prints one per line; it does not mutate session data. An empty or unreadable root prints <code>No sessions found.</code>.",
    refusals: [
      "There is no command-specific hard refusal: a non-numeric limit is ignored because parsing uses an optional integer conversion.",
      "Unknown global options are refused before dispatch.",
    ],
  },
  {
    path: "show",
    invocation: "jeden show <session-id-or-path>",
    purpose: "Render one durable session export as JSON on stdout.",
    inputs: ["Required: a session identifier under the session root or a session directory path containing a slash."],
    effect: "Reads the session state and validated transcript ledger, then prints its id, path, ledger version, active leaf, recovery flag, and exported events. It does not mutate the session.",
    refusals: [
      "A missing selector is refused as <code>show requires a session id</code>.",
      "A missing or unreadable session is represented in the printed JSON error object by this dispatcher rather than changing files.",
    ],
  },
  {
    path: "export",
    invocation: "jeden export <session-id-or-path> [output] [--html|--markdown]",
    purpose: "Export a recorded session as JSON, HTML, or Markdown.",
    inputs: [
      "Required: a session identifier or path.",
      "Optional: <code>--html</code> or <code>--markdown</code>; JSON is the default. A non-flag trailing value is the output file.",
    ],
    effect: "Prints the serialized session when no output path is supplied; otherwise writes the payload to that path and prints the path.",
    refusals: [
      "A missing selector is refused as <code>export requires a session id or path</code>.",
      "Missing sessions, invalid ledgers, unsupported renderer formats, and output write errors are returned without a partial successful result.",
    ],
  },
  {
    path: "artifacts",
    invocation: "jeden artifacts <session-id-or-path>",
    purpose: "List files in one session's artifact directory.",
    inputs: ["Required: a session identifier or path."],
    effect: "Prints sorted <code>name&lt;TAB&gt;byte-size</code> rows for regular artifact files and prints nothing when the directory has no readable files. It does not mutate state.",
    refusals: [
      "A missing selector is refused as <code>artifacts requires a session id</code>.",
      "Unreadable or absent artifact directories yield an empty listing rather than fabricating entries.",
    ],
  },
  {
    path: "artifact",
    invocation: "jeden artifact <session-id-or-path> <name> [output]",
    purpose: "Read one UTF-8 session artifact or copy it to a requested output file.",
    inputs: [
      "Required: session identifier or path and artifact name.",
      "Optional: an output path. Without it, artifact text is printed and normalized to end with a newline.",
    ],
    effect: "Canonicalizes the session artifact root and selected file, reads the artifact as text, and either prints it or writes the same text to the output path.",
    refusals: [
      "Missing selectors are refused as <code>artifact requires a session id or path</code> or <code>artifact requires an artifact name</code>.",
      "A canonical artifact path outside the session root is refused as <code>artifact path escapes session: &lt;name&gt;</code>.",
      "Missing, non-UTF-8, or unwritable files return their filesystem error.",
    ],
  },
  {
    path: "tools",
    invocation: "jeden tools [--json] [--cwd path]",
    purpose: "Inspect the currently visible and executable Jeden tool registry.",
    inputs: [
      "Optional: <code>--cwd</code> to select project tools and configuration.",
      "Optional: <code>--json</code> for structured tool name, description, and input-schema rows; the default is a text table.",
    ],
    effect: "Discovers built-ins, project/user custom tools, MCP tools, and their capability health, then prints the active projection without executing a tool.",
    refusals: [
      "Unknown global options are refused before discovery.",
      "Unavailable or conflicting tools are excluded or surfaced through capability diagnostics rather than advertised as executable.",
    ],
  },
  {
    path: "search-sessions",
    invocation: 'jeden search-sessions "query" [limit]',
    purpose: "Search durable session event payloads for a case-insensitive text fragment.",
    inputs: [
      "Required: a non-empty query as the first positional value.",
      "Optional: a numeric session scan limit as the second positional value.",
    ],
    effect: "Scans newest session directories first and prints at most one tab-separated matching event row per scanned session: id, timestamp, event type, and whitespace-collapsed event JSON. It does not mutate sessions.",
    refusals: [
      "Missing and blank queries are refused as <code>search-sessions requires a query</code> and <code>search-sessions requires a non-empty query</code>.",
      "An unreadable matching session is reported as <code>cannot search session ...</code>; a non-numeric limit is ignored.",
    ],
  },
  {
    path: "resume",
    invocation: 'jeden resume <session-id-or-path> ["task"] [--allow-write] [--allow-command] [--yolo|--auto-approve]',
    purpose: "Seed a fresh Jeden session with the conversation turns from a recorded session and optionally continue it immediately.",
    inputs: [
      "Required: a session identifier or path.",
      "Optional: a task to run after loading history and explicit write/command grants for that continued turn.",
    ],
    effect: "Creates a new conversation, loads replayable history, and either prints how to continue or executes the supplied task, records a new session, and prints the resumed result.",
    refusals: [
      "Missing input is refused with <code>Usage: jeden resume &lt;session-id-or-path&gt; [\"&lt;task&gt;\"]</code>.",
      "A nonexistent source is refused as <code>session not found: ...</code>; invalid session ledgers or model/tool errors also stop continuation.",
    ],
  },
  {
    path: "recall_conversation",
    invocation: "jeden recall_conversation <session-id-or-path>",
    purpose: "Render a recorded session's full event transcript as Markdown for recall or inspection.",
    inputs: [
      "Required: a session identifier or path.",
      "The dispatcher also accepts the compatibility spelling <code>recall-conversation</code>; <code>recall_conversation</code> is the documented invocation.",
    ],
    effect: "Reads the validated ledger and prints a Markdown document containing the session identity and each exported event. It does not mutate the source session.",
    refusals: [
      "Missing input is refused with <code>Usage: jeden recall_conversation &lt;session-id-or-path&gt;</code>.",
      "A missing session is refused as <code>session not found: ...</code>; invalid ledger data is returned as an error.",
    ],
  },
  {
    path: "update",
    invocation: "JEDEN_UPDATE_MANIFEST=<https-or-local-dsse-manifest> jeden update",
    purpose: "Transactionally install a signed Jeden release and verify the activated binary before committing it.",
    inputs: [
      "Required environment: <code>JEDEN_UPDATE_MANIFEST</code>, pointing to an HTTPS or local DSSE release manifest.",
      "Optional environment: <code>JEDEN_UPDATE_CHANNEL</code> (<code>stable</code> by default), <code>JEDEN_UPDATE_TARGET_TRIPLE</code>, and <code>JEDEN_UPDATE_TARGET</code>.",
    ],
    effect: "Recovers any prior update journal, verifies the DSSE signature against the embedded channel trust root, checks target/version/digests plus SBOM and provenance, installs atomically, runs post-health, and prints the installed version and digest. Failed health rolls back to the last-known-good binary.",
    refusals: [
      "Absent manifest configuration is refused exactly as <code>JEDEN_UPDATE_MANIFEST must point to an HTTPS or local DSSE release manifest</code>.",
      "Channels other than <code>canary</code> or <code>stable</code>, noncanonical or unsigned manifests, mismatched targets/digests/evidence, unsafe archives, downgrade/selection failures, and missing rollback material are refused.",
    ],
  },
  {
    path: "config",
    invocation: "jeden config [list|path|get <key>|set <key> <value>|reset <key>] [--json] [--cwd path]",
    purpose: "Inspect and change Jeden's schema-backed user configuration.",
    inputs: [
      "No action defaults to <code>list</code>.",
      "<code>--cwd</code> selects the project layer used when computing effective values; <code>--json</code> selects structured output.",
      "Use the linked leaf commands for their required key/value inputs.",
    ],
    effect: "Lists merged effective settings, prints the writable user path, reads one setting, or atomically writes a schema-validated user value/default depending on the selected action.",
    refusals: [
      "Unknown actions are refused with the exact usage line shown above.",
      "Unknown keys and values that do not match the setting's boolean, finite-number, enum, array, object, or string schema are refused before writing.",
    ],
  },
  {
    path: "doctor",
    invocation: "jeden doctor [--json] [--cwd path]",
    purpose: "Probe the live health of Jeden's configured runtime dependencies and local subsystems.",
    inputs: ["Optional: <code>--cwd</code>. The command always emits its structured doctor report; <code>--json</code> is accepted for CLI consistency."],
    effect: "Runs Brama, Weles, storage, process, MCP, extensions, LSP, browser, TUI keymap, task, memory, and collaboration probes, then prints a JSON report with per-probe evidence and latency.",
    refusals: [
      "The command exits unsuccessfully when any active probe is unavailable; degraded or inactive evidence remains explicit in the report.",
      "Storage probe failures and serialization failures are returned as errors instead of a healthy result.",
    ],
  },
  {
    path: "conformance",
    invocation: "jeden conformance [--json] [--cwd path]",
    purpose: "Evaluate Jeden's canonical completion areas, production scopes, evidence, and UI-honesty contract.",
    inputs: ["Optional: <code>--cwd</code>. Output is canonical compact JSON; <code>--json</code> is accepted but not required."],
    effect: "Reads source/inventory evidence, computes every area and production-scope status plus UI-honesty findings, sorts the report deterministically, and prints it without changing product state.",
    refusals: [
      "The command exits unsuccessfully when the report's <code>complete</code> field is false, including missing evidence, failed behavior/contracts, incomplete production scopes, or UI-honesty findings.",
      "Report construction or canonical serialization errors are returned and no passing verdict is emitted.",
    ],
  },
  {
    path: "probierz",
    invocation: "jeden probierz [args...]",
    purpose: "Run Probierz discovery, evidence, and gate commands with the active Jeden executable and model configuration.",
    inputs: [
      "Optional: arguments forwarded verbatim to Probierz. With none, Jeden runs <code>probierz status jeden --text</code>.",
      "<code>PROBIERZ_ROOT</code> may select a source checkout; otherwise a sibling checkout is preferred and then the installed <code>probierz</code> executable.",
    ],
    effect: "Sets <code>TUI_CMD</code> to the current Jeden executable when absent, propagates the selected model as <code>JEDEN_MODEL</code> when needed, and lets Probierz own its reports, artifacts, and gate output.",
    refusals: [
      "Launch failure is reported with the instruction to set <code>PROBIERZ_ROOT</code> or install the CLI.",
      "Any non-success Probierz status is returned as <code>Probierz exited with ...</code>; Jeden never converts a failed gate into success.",
    ],
  },
  {
    path: "capabilities",
    invocation: "jeden capabilities [--json] [--cwd path]",
    purpose: "Inspect Jeden's atomic capability-discovery and health snapshot.",
    inputs: ["Optional: <code>--cwd</code> for project capability discovery and <code>--json</code> for the full versioned descriptor snapshot."],
    effect: "Discovers tools, slash commands, views, extensions, plugins, MCP servers, skills, agents, rules, and services; prints per-kind availability in text or the complete descriptor, binding, health, diagnostics, and generation data in JSON.",
    refusals: [
      "Conflicts and unavailable capabilities are reported as diagnostics and are not exposed as executable.",
      "JSON serialization failure or a poisoned rebuild lock is returned as an error rather than a partial healthy snapshot.",
    ],
  },
  {
    path: "completions",
    invocation: "jeden completions <bash|zsh|fish>",
    purpose: "Generate a shell-completion program from Jeden's current CLI usage and builtin slash-command registry.",
    inputs: ["Required: exactly one supported shell name: <code>bash</code>, <code>zsh</code>, or <code>fish</code>."],
    effect: "Prints the completion script to stdout. The model is derived from the in-repo usage and capability tables so commands, global flags, command flags/actions, and slash words stay aligned.",
    refusals: ["A missing or unknown shell is refused as <code>unknown shell '&lt;missing-or-value&gt;': usage: jeden completions &lt;bash|zsh|fish&gt;</code>."],
  },
  {
    path: "worktree",
    invocation: "jeden worktree [list|clear] [--dry-run] [--json] [--cwd path]",
    purpose: "Inspect or safely clear stale Git worktrees owned by Jeden's task runtime.",
    inputs: [
      "No action defaults to <code>list</code>.",
      "<code>--dry-run</code> previews <code>clear</code>; <code>--json</code> selects structured rows and <code>--cwd</code> selects the repository.",
    ],
    effect: "Lists task-record-correlated managed worktrees, or removes only stale worktrees that pass canonical managed-root, checkout, and repository-top safety checks.",
    refusals: [
      "Unexpected arguments are refused with <code>usage: jeden worktree [list|clear] [--dry-run] [--json]</code>.",
      "Clear skips the current checkout, repository root, live/unknown work, and any path outside Jeden-managed workspace roots; it reports the reason instead of removing it.",
    ],
  },
  {
    path: "token",
    invocation: "jeden token [--list] [--reveal] [--json]",
    purpose: "Print the agent's own Brama credential for shell scripting, redacted unless explicitly revealed.",
    inputs: [
      "Required environment: non-empty <code>BRAMA_URL</code> and <code>WISENT_APP_AGENT_AUTH_SECRET</code>. <code>WISENT_APP_AGENT_ID</code> is included when configured.",
      "<code>--reveal</code> prints the bare secret; <code>--list</code> adds Weles accounts to text output; <code>--json</code> returns the structured URL, agent id, and redacted or revealed token.",
    ],
    effect: "Reads credentials from process memory and prints them; it does not persist, rotate, or revoke credentials. Default text reveals only the final four characters and length.",
    refusals: [
      "Missing router URL is refused as <code>BRAMA_URL is required; configure the Brama model-router service URL</code>.",
      "Missing agent secret is refused as <code>WISENT_APP_AGENT_AUTH_SECRET is not configured; launch with bin/jeden-rust or scripts/run-with-stado.sh</code>.",
    ],
  },
  {
    path: "stats",
    invocation: "jeden stats [--json|--summary|--serve [--port N]]",
    purpose: "Show local usage, quota, and session totals or serve the same snapshot as a loopback dashboard.",
    inputs: [
      "No mode prints the full text snapshot; <code>--json</code> prints structured data and <code>--summary</code> prints one project summary line.",
      "<code>--serve</code> binds <code>127.0.0.1</code>; <code>--port N</code> selects a <code>u16</code> port, defaulting to 3847 when absent or unparsable.",
    ],
    effect: "Reads project/user usage ledgers, platform quota availability, and recent local sessions. Serve mode exposes only <code>/</code> and <code>/api/stats</code> on loopback until stopped.",
    refusals: [
      "Serve mode refuses a loopback bind failure as <code>cannot bind 127.0.0.1:&lt;port&gt;: ...</code>.",
      "Unknown HTTP paths return 404. Unavailable quota is reported in the snapshot rather than represented as available.",
    ],
  },
  {
    path: "gallery",
    invocation: "jeden gallery [--theme NAME|--all] [--color]",
    purpose: "Render the TUI component gallery under the effective theme or every bundled preset.",
    inputs: [
      "Optional: <code>--theme NAME</code> for one preset, <code>--all</code> for every preset, and <code>--color</code> to force ANSI color output.",
      "With no theme option, the currently effective theme is used.",
    ],
    effect: "Prints deterministic fixture views for messages, pickers, tables, tabs, confirmations, progress, markdown, diff, QR, and related TUI components. Temporary theme environment changes are restored before return.",
    refusals: ["An unknown requested theme is refused as <code>unknown theme `NAME`; bundled presets: ...</code>."],
  },
  {
    path: "roadmap",
    invocation: "jeden roadmap <list|show|add|drop|start|implemented|block|pass|status|depends|undepends|graph|acceptance|check|work> [args] [--json] [--cwd path]",
    purpose: "Read and transactionally mutate the repository-owned Jeden roadmap.",
    inputs: [
      "Required for non-default use: one listed action; no action defaults to <code>list</code>.",
      "Mutation leaves accept <code>--revision n</code> for optimistic concurrency; without it they load the current revision.",
      "Use the linked leaf pages for action-specific identifiers, values, filters, reasons, evidence, and output.",
    ],
    effect: "Reads or atomically updates <code>roadmap/roadmap.yaml</code>, validates the complete graph and status invariants, increments the revision for mutations, and appends roadmap event evidence.",
    refusals: [
      "Unknown actions are refused with the complete expected command set.",
      "A stale <code>--revision</code> is refused as a revision conflict; invalid YAML, schema, IDs, dependencies, priorities, statuses, evidence, or atomic writes never commit a partial mutation.",
    ],
  },

  // Config leaves.
  {
    path: "config/list",
    invocation: "jeden config list [--json] [--cwd path]",
    purpose: "List every schema-backed setting and its effective value.",
    inputs: ["Optional: <code>--cwd</code> for merged project-over-user values and <code>--json</code> for metadata objects."],
    effect: "Prints grouped text rows with value, type, and description, or JSON keyed by setting with value, type, description, default, and enum choices. It does not write configuration.",
    refusals: ["Unknown global options are refused; unreadable configuration falls through the existing loader's safe defaults rather than inventing an unsupported key."],
  },
  {
    path: "config/path",
    invocation: "jeden config path",
    purpose: "Print the writable user configuration file path.",
    inputs: ["No key or value is accepted or required."],
    effect: "Prints <code>~/.jeden/config.yml</code> using the resolved home directory. It does not create or modify the file.",
    refusals: ["An unknown action at the config level is refused with the config usage contract; unknown global options are refused before dispatch."],
  },
  {
    path: "config/get",
    invocation: "jeden config get <key> [--json] [--cwd path]",
    purpose: "Read one effective schema-backed setting.",
    inputs: ["Required: an exact key from the setting schema. Optional: <code>--json</code> for metadata and <code>--cwd</code> for the project layer."],
    effect: "Prints the effective string or JSON value; JSON mode also returns type, description, default, and enum metadata. It does not write configuration.",
    refusals: ["Missing input is refused as <code>config get requires a key</code>; an unregistered key is refused as <code>unknown config key: &lt;key&gt;</code>."],
  },
  {
    path: "config/set",
    invocation: "jeden config set <key> <value> [--json]",
    purpose: "Validate and persist one user configuration setting.",
    inputs: ["Required: exact schema key and value. Multi-token values are joined with spaces before type parsing; JSON arrays and records must be valid JSON."],
    effect: "Parses the schema type, updates the nested user configuration object, atomically writes the user config, and prints the key/path or structured mutation result.",
    refusals: [
      "Missing input is refused as <code>config set requires a key</code> or <code>config set requires a value</code>; unknown keys are refused.",
      "Invalid booleans, non-finite numbers, out-of-enum values, non-array JSON, and non-object JSON are refused with a key-specific message before writing.",
    ],
  },
  {
    path: "config/reset",
    invocation: "jeden config reset <key> [--json]",
    purpose: "Persist one setting's schema default into the user configuration.",
    inputs: ["Required: an exact schema key. Optional: <code>--json</code> for the key, default value, type, description, and path."],
    effect: "Sets the nested user key to the schema default, atomically writes the user config, and prints the reset result.",
    refusals: ["Missing input is refused as <code>config reset requires a key</code>; an unregistered key is refused as <code>unknown config key: &lt;key&gt;</code>."],
  },

  // Completion leaves.
  ...["bash", "zsh", "fish"].map((shell) => ({
    path: `completions/${shell}`,
    invocation: `jeden completions ${shell}`,
    purpose: `Generate Jeden completions for ${shell}.`,
    inputs: [`Required shell selector: <code>${shell}</code>; no output path is accepted because the script is written to stdout.`],
    effect: `Prints a ${shell} completion program derived from the current usage and slash-command tables; redirect it into the shell's normal completion-loading location.`,
    refusals: ["Any selector outside <code>bash</code>, <code>zsh</code>, and <code>fish</code> is refused by the parent command's exact unknown-shell error."],
  })),

  // Worktree leaves.
  {
    path: "worktree/list",
    invocation: "jeden worktree list [--json] [--cwd path]",
    purpose: "List Git worktrees correlated with Jeden task records.",
    inputs: ["Optional: <code>--cwd</code> for the repository and <code>--json</code> for structured rows."],
    effect: "Reads Git worktree metadata and task stores, classifies managed entries as live or stale, and prints them without creating scheduler state or mutating a worktree.",
    refusals: ["Unexpected positional values are refused; repositories without managed worktrees return an explanatory empty-state result."],
  },
  {
    path: "worktree/clear",
    invocation: "jeden worktree clear [--dry-run] [--json] [--cwd path]",
    purpose: "Remove stale, safely bounded Git worktrees owned by Jeden.",
    inputs: ["Optional: <code>--dry-run</code>, <code>--json</code>, and repository <code>--cwd</code>."],
    effect: "Evaluates stale managed worktrees and either reports the removal plan or runs <code>git worktree remove</code>, returning removed and skipped rows with reasons.",
    refusals: [
      "The command refuses removal outside canonical Jeden-managed roots and never removes the current checkout or repository top level.",
      "Live or unknown task ownership is kept; Git removal failures are reported as skipped, not silently counted as removed.",
    ],
  },

  // Roadmap direct leaves.
  {
    path: "roadmap/list",
    invocation: "jeden roadmap list [--status STATUS] [--area AREA] [--priority PRIORITY] [--json]",
    purpose: "List roadmap items, optionally filtered by exact status, area, or priority.",
    inputs: ["Optional filters: <code>--status</code>, <code>--area</code>, and <code>--priority</code>; optional structured <code>--json</code> output."],
    effect: "Reads the roadmap revision and prints matching item rows or a JSON envelope. It does not mutate the roadmap.",
    refusals: ["Unknown status values are refused with the complete allowed status set; a missing option value is refused as <code>--&lt;name&gt; requires a value</code>."],
  },
  {
    path: "roadmap/show",
    invocation: "jeden roadmap show <id> [--json]",
    purpose: "Show one roadmap item by case-insensitive ID.",
    inputs: ["Required: roadmap item ID. Optional: <code>--json</code>."],
    effect: "Prints the item as JSON or as the roadmap's Markdown rendering without changing it.",
    refusals: ["Missing input is refused with <code>Usage: roadmap show &lt;id&gt;</code>; an unknown ID is returned as not found."],
  },
  {
    path: "roadmap/add",
    invocation: "jeden roadmap add <title> | --title <title> [--area AREA] [--priority P0|P1|P2|P3] [--summary TEXT] [--acceptance TEXT] [--depends-on ID] [--capability ID] [--external-prerequisite TEXT] [--status STATUS] [--revision N] [--json]",
    purpose: "Create a new validated roadmap item.",
    inputs: [
      "Required: a positional title or <code>--title</code>, but not both.",
      "Optional metadata includes explicit id, area, priority, summary, implementation/rationale/order, repeatable acceptance, dependencies, capabilities, prerequisites, initial status, and expected revision.",
    ],
    effect: "Builds the item with actor/timestamp metadata, assigns the requested or next ID, validates the entire roadmap, atomically commits a new revision, and prints the created item or ID/revision.",
    refusals: [
      "Supplying both title forms is refused; an empty title returns the exact add usage.",
      "Invalid priority, status, duplicate or missing IDs/dependencies, invalid acceptance/evidence, cycles, and stale revisions are refused without committing.",
    ],
  },
  ...[
    ["drop", "jeden roadmap drop <id> [reason|--reason TEXT] [--revision N] [--json]", "Mark an item dropped.", "Sets status to dropped, records an optional reason and roadmap event, and commits a new revision.", "The ID is required; drop is refused when another roadmap item depends on the target."],
    ["start", "jeden roadmap start <id> [reason|--reason TEXT] [--revision N] [--json]", "Mark an item in progress.", "Sets status to in_progress, records an optional reason and roadmap event, and commits a new revision.", "The ID is required; unknown IDs, invalid roadmap state, and stale revisions are refused."],
    ["implemented", "jeden roadmap implemented <id> [reason|--reason TEXT] [--revision N] [--json]", "Mark an item implemented but not yet passed.", "Sets status to implemented, records an optional reason and roadmap event, and commits a new revision.", "The ID is required; unknown IDs, invalid roadmap state, and stale revisions are refused."],
    ["pass", "jeden roadmap pass <id> [reason|--reason TEXT] [--evidence URI] [--revision N] [--json]", "Mark an item passed with evidence.", "Sets status to passed, appends repeatable evidence URIs, records the event, validates, and commits a new revision.", "The ID is required, and roadmap validation refuses passed items without any evidence."],
  ].map(([name, invocation, purpose, effect, refusal]) => ({
    path: `roadmap/${name}`,
    invocation,
    purpose,
    inputs: ["Required: roadmap item ID. Optional reason, expected revision, JSON output, and action-specific values shown in the invocation."],
    effect,
    refusals: [refusal, "Every mutation is refused on a revision conflict or when full-roadmap validation fails."],
  })),
  {
    path: "roadmap/block",
    invocation: "jeden roadmap block <id> <reason> [--external-prerequisite TEXT] [--evidence URI] [--revision N] [--json]",
    purpose: "Mark an item externally blocked and record what must change outside the repository.",
    inputs: ["Required: item ID plus a positional reason or at least one <code>--external-prerequisite</code>. Optional evidence, revision, and JSON output."],
    effect: "Sets status to external_blocked, merges prerequisites, appends evidence, records a blocked event, validates, and commits a new revision.",
    refusals: ["A missing reason and missing prerequisite are refused as <code>roadmap block requires a reason or --external-prerequisite</code>; unknown IDs and stale revisions are refused."],
  },
  {
    path: "roadmap/status",
    invocation: "jeden roadmap status <id> <status> [reason|--reason TEXT] [--external-prerequisite TEXT] [--evidence URI] [--revision N] [--json]",
    purpose: "Set an item's explicit roadmap status.",
    inputs: ["Required: item ID and one allowed status. Optional reason, prerequisites, evidence, revision, and JSON output."],
    effect: "Applies the selected status through the shared status mutation, records status-specific event data, validates, and commits a new revision.",
    refusals: ["Missing values return <code>Usage: roadmap status &lt;id&gt; &lt;status&gt;</code>; unknown statuses list the allowed set, and external_blocked still requires a reason or prerequisite."],
  },
  ...[
    ["depends", "jeden roadmap depends <id> <dependency-id> [--revision N] [--json]", "Add a dependency edge.", "Adds the uppercased dependency ID, validates the complete graph, records an update event, and commits."],
    ["undepends", "jeden roadmap undepends <id> <dependency-id> [--revision N] [--json]", "Remove a dependency edge.", "Removes the matching dependency edge, validates, records an update event, and commits."],
  ].map(([name, invocation, purpose, effect]) => ({
    path: `roadmap/${name}`,
    invocation,
    purpose,
    inputs: ["Required: source item ID and dependency item ID. Optional expected revision and JSON output."],
    effect,
    refusals: [
      `Missing values return <code>Usage: roadmap ${name} &lt;id&gt; &lt;dependency-id&gt;</code>; unknown items and stale revisions are refused.`,
      name === "depends" ? "Self-dependencies, missing targets, duplicate IDs, and cycles are refused by full-roadmap validation." : "Removing an edge that is not present is refused as <code>&lt;id&gt; does not depend on &lt;dependency-id&gt;</code>.",
    ],
  })),
  {
    path: "roadmap/graph",
    invocation: "jeden roadmap graph [--json]",
    purpose: "Render the roadmap dependency graph.",
    inputs: ["Optional: <code>--json</code> for structured nodes, edges, and revision."],
    effect: "Reads and prints every roadmap node and dependency edge, or the JSON graph. It does not mutate state.",
    refusals: ["Invalid or unreadable roadmap data is refused; the command never emits a success graph from a parse failure."],
  },
  {
    path: "roadmap/acceptance",
    invocation: "jeden roadmap acceptance <list|add|evidence> <item-id> ... [--revision N] [--json]",
    purpose: "Inspect acceptance criteria or attach new criteria and criterion-specific evidence.",
    inputs: ["Required: operation and item ID, followed by the criterion text or criterion/evidence identifiers required by the selected leaf."],
    effect: "List is read-only; add/evidence validate and atomically commit a roadmap revision with an update or evidence event.",
    refusals: ["Missing group inputs return the exact acceptance usage; unknown operations are refused as <code>unknown acceptance operation: ...</code>."],
  },
  {
    path: "roadmap/acceptance/list",
    invocation: "jeden roadmap acceptance list <item-id> [--json]",
    purpose: "List one item's acceptance criteria and attached evidence counts.",
    inputs: ["Required: item ID. Optional: <code>--json</code>."],
    effect: "Prints criterion IDs, text, and evidence counts, or structured acceptance and evidence arrays. It does not mutate state.",
    refusals: ["Missing item input returns the parent acceptance usage; an unknown item is returned as not found."],
  },
  {
    path: "roadmap/acceptance/add",
    invocation: "jeden roadmap acceptance add <item-id> <criterion> [--id ID] [--revision N] [--json]",
    purpose: "Append an acceptance criterion to one roadmap item.",
    inputs: ["Required: item ID and non-empty criterion text. Optional explicit criterion ID, expected revision, and JSON output."],
    effect: "Assigns the requested or next acceptance ID, appends the criterion, records an update event, validates, and commits a new revision.",
    refusals: ["Empty criterion text returns <code>Usage: roadmap acceptance add &lt;item-id&gt; &lt;criterion&gt;</code>; unknown items, duplicate/invalid acceptance data, and stale revisions are refused."],
  },
  {
    path: "roadmap/acceptance/evidence",
    invocation: "jeden roadmap acceptance evidence <item-id> <acceptance-id> <artifact-uri> [--revision N] [--json]",
    purpose: "Attach one artifact URI to a specific acceptance criterion.",
    inputs: ["Required: item ID, acceptance ID, and artifact URI. Optional expected revision and JSON output."],
    effect: "Appends timestamped actor-owned evidence, records <code>roadmap_evidence_attached</code>, validates, and commits a new revision.",
    refusals: ["Missing values return the exact evidence usage; an unknown criterion is refused as <code>&lt;item&gt; has no acceptance criterion &lt;id&gt;</code>, and stale revisions are refused."],
  },
  {
    path: "roadmap/check",
    invocation: "jeden roadmap check [--json]",
    purpose: "Validate the complete roadmap without mutating it.",
    inputs: ["Optional: <code>--json</code> for the structured validation report."],
    effect: "Checks schema, revision, item uniqueness, priority/status, acceptance/evidence references, pass/block invariants, dependencies, and graph integrity; prints the report or a valid summary.",
    refusals: ["Any validation error makes text mode return the joined errors instead of <code>Roadmap valid</code>; unreadable or invalid YAML also fails."],
  },
  {
    path: "roadmap/work",
    invocation: "jeden roadmap work <item-id> [--json]",
    purpose: "Activate one eligible roadmap item as the current goal, plan, todo set, and session work context.",
    inputs: ["Required: roadmap item ID. Optional: <code>--json</code>."],
    effect: "Loads the item, creates goal/plan/todos from its acceptance criteria, activates roadmap context, records a roadmap_item_started session event, and prints the new session path and todo count.",
    refusals: [
      "Missing input returns <code>Usage: roadmap work &lt;item-id&gt;</code>.",
      "Dropped or passed items are refused as not workable, and any dependency not passed refuses activation as <code>&lt;id&gt; is blocked by unresolved dependencies: ...</code>.",
    ],
  },
];

const groups = [
  ["Run and automation", ["run", "pursue", "rpc", "headless", "acp", "collab-relay"]],
  ["Sessions and artifacts", ["sessions", "show", "export", "artifacts", "artifact", "search-sessions", "resume", "recall_conversation"]],
  ["Runtime and operations", ["tools", "update", "doctor", "conformance", "probierz", "capabilities", "token", "stats", "gallery"]],
  ["Configuration", ["config", "config/list", "config/path", "config/get", "config/set", "config/reset"]],
  ["Shell completions", ["completions", "completions/bash", "completions/zsh", "completions/fish"]],
  ["Managed worktrees", ["worktree", "worktree/list", "worktree/clear"]],
  ["Roadmap", [
    "roadmap", "roadmap/list", "roadmap/show", "roadmap/add", "roadmap/drop", "roadmap/start", "roadmap/implemented", "roadmap/block", "roadmap/pass", "roadmap/status", "roadmap/depends", "roadmap/undepends", "roadmap/graph", "roadmap/acceptance", "roadmap/acceptance/list", "roadmap/acceptance/add", "roadmap/acceptance/evidence", "roadmap/check", "roadmap/work",
  ]],
];

const byPath = new Map(commands.map((entry) => [entry.path, entry]));
for (const [, paths] of groups) {
  for (const path of paths) {
    if (!byPath.has(path)) throw new Error(`CLI navigation references unknown command path: ${path}`);
  }
}
if (new Set(groups.flatMap(([, paths]) => paths)).size !== commands.length) {
  throw new Error("CLI command tree must link every command page exactly once");
}

export const cliRouteContract = commands.map(({ path, invocation }) => ({
  path: `/docs/cli/${path}`,
  invocation,
}));

export const cliIndexPage = {
  slug: "cli",
  navLabel: "CLI reference",
  href: "/docs/cli",
  file: "cli.html",
  meta: {
    htmlTitle: "CLI reference — Jeden documentation",
    description: "Complete source-grounded Jeden CLI command tree, with one canonical page per command and leaf subcommand.",
    ogTitle: "CLI reference — Jeden documentation",
    ogDescription: "Every public Jeden CLI command, invocation, input, effect, and refusal.",
    canonical: `${ORIGIN}/docs/cli`,
  },
  eyebrow: "Command reference",
  title: "The complete <em>Jeden CLI.</em>",
  description: "Every command below is dispatched by the current Jeden binary. Each linked page gives the exact invocation, purpose, required inputs and options, output or state effect, and the refusal boundaries enforced by source.",
  sections: [
    {
      title: "Interactive root and global options",
      paragraphs: [
        "Running <code>jeden</code> without a command opens the interactive terminal. <code>--cwd path</code>, <code>--model name</code>, <code>--max-tokens n</code>, <code>--max-steps n</code>, <code>--allow-write</code>, <code>--allow-command</code>, and <code>--yolo</code>/<code>--auto-approve</code> configure that root invocation. <code>--version</code>/<code>-V</code> prints the compiled version and <code>--help</code>/<code>-h</code> prints usage.",
        "Unknown commands and unknown global options fail instead of falling through. Environment files load from the selected workspace before dispatched commands run.",
      ],
      commands: [{ label: "Interactive", code: "jeden [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]" }],
    },
    ...groups.map(([title, paths]) => ({
      title,
      bullets: paths.map((path) => {
        const entry = byPath.get(path);
        return `<a href=\"/docs/cli/${path}\"><code>jeden ${path.split("/").join(" ")}</code></a> — ${entry.purpose}`;
      }),
    })),
  ],
};

export const cliPages = [cliIndexPage, ...commands.map(commandPage)];
