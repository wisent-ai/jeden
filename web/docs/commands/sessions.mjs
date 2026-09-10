export const sessionCommands = [
  {
    path: "run",
    invocation: 'jeden run "task" [--json] [--model-only] [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]',
    purpose: "Run one concrete agent task through a durable Jeden conversation.",
    inputs: [
      "Required: a non-empty task after <code>run</code>.",
      "<code>--cwd</code>, <code>--model</code>, <code>--max-tokens</code>, and <code>--max-steps</code> select the workspace, route, response budget, and step bound.",
      "<code>--allow-write</code> and <code>--allow-command</code> grant those tool tiers; <code>--yolo</code>/<code>--auto-approve</code> grants both. <code>--model-only</code> suppresses tools and <code>--json</code> wraps local slash-command output where supported.",
    ],
    effect: "Records the original request durably before model access, then records acceptance requirements in a separate read-only conversation before execution. Every proposed final is independently checked against all retained tasks and original requests; incomplete actionable work continues automatically. The returned JSON includes <code>completion</code> with status, revision, requests, tasks and blockers. Model-only calls and Pursuit's independently owned stage reviews keep their own output contracts.",
    refusals: [
      "A missing task is refused exactly as <code>run requires a task</code>.",
      "A missing value for a valued option is refused (for example <code>--model requires a value</code>); non-integer token or step bounds are refused.",
      "Model work fails closed when required router configuration or credentials are absent, and write or command tools remain approval-gated unless their tier was granted.",
      "On macOS, a missing task sandbox helper is reported as <code>jeden-sandbox-helper is not installed beside the Jeden executable; build and code-sign it or set JEDEN_TASK_SANDBOX_HELPER</code>. Install the complete release, keeping its signed <code>jeden-sandbox-helper</code> beside <code>jeden</code>; the release SBOM and provenance identify both executables.",
      "A non-rate-limit HTTP failure from Brama includes its status, the requested API path, and the quoted response body. An empty quoted body means that the upstream server supplied no error detail; it is not a model answer. The CLI and RPC report the same failure.",
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
      "<code>session/completion/get</code> takes <code>{sessionId}</code>; <code>session/completion/control</code> takes <code>{sessionId,taskId,action,reason,revision}</code>, with action pause, resume or cancel. Both return <code>{sessionId,completion}</code>. Task IDs may also name an original request.",
      "<code>session/completion/continue</code> takes <code>{sessionId,requestId}</code> and uses the ordinary prompt response and event stream without capturing a new user request. Events <code>completion {state}</code> and <code>assistantMessage {text}</code> are nonterminal; <code>result</code> includes text and the completion snapshot.",
    ],
    effect: "Keeps an in-process session map, emits one-line JSON responses and interaction events, and creates normal Jeden session state for requests that start or resume work. User-facing session prompts use the same structured task-report contract as CLI/TUI, headless, and the SDKs; accepted <code>task_report</code> events carry version, complete-or-blocked status, the seven-area report, and its rendered human text. The <code>config/contracts/get</code> and <code>config/contracts/set</code> results include the two editable user contract settings plus a localized, versioned, built-in read-only <code>taskContract</code> with instructions and requirement descriptors. <code>config/communication/get</code> and <code>config/communication/set</code> return the resolved <code>effective</code> communication policy. Session events include <code>toolCall</code>, <code>toolResult</code>, and <code>reasoningDelta</code> only when that policy shows them. RPC opens no listener.",
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
      "Each identity-map entry must supply <code>san</code>, <code>principal</code>, and <code>tenant</code>, and may supply <code>workspaces</code>: a list of absolute host directories that principal is granted.",
      "The same completion get, control and continue methods are available after session creation or open, subject to the existing tenant/workspace authorization. Continue uses the request metadata idempotency key and returns the same started, reattached or completed response as session/prompt.",
    ],
    effect: "Creates <code>.jeden/headless</code> service state, a durable reconnect key, tenant idempotency/replay stores, and an mTLS listener at the requested address. Beyond <code>health/readiness</code>, <code>session/create</code>, <code>session/reconnect</code>, <code>session/prompt</code>, <code>session/replay</code> and <code>session/cancel</code>, it serves <code>session/list</code>, <code>session/open</code> and <code>session/history</code>: a principal with granted <code>workspaces</code> lists the host's own sessions whose recorded <code>cwd</code> lies inside a grant, opens one under its own host session id so later prompts continue that very ledger, and reads its replayed turns without resuming it. A principal without <code>workspaces</code> keeps seeing only the sessions it created here, jailed to its tenant scratch workspace.",
    refusals: [
      "Any argument count other than five or six is refused with the exact usage line shown above.",
      "Unreadable or invalid identity maps, empty maps, invalid mappings, TLS material failures, revoked certificates, certificates without an identity SAN, and bind failures are refused.",
      "A <code>workspaces</code> entry that is not an absolute path, contains <code>..</code>, or is not an existing readable directory is named and refused when the identity map is loaded.",
      "<code>session/open</code> and <code>session/history</code> refuse <code>access_denied</code> for a principal without granted workspaces, for an id that is not a session directory under the session root, and for a session whose recorded <code>cwd</code> lies outside every grant; a missing or blank <code>sessionId</code> and a <code>limit</code> below one are <code>invalid_request</code>.",
      "A session directory whose <code>state.json</code> cannot be read is left out of <code>session/list</code> and counted in the reply's <code>skipped</code> field rather than silently dropped.",
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
    effect: "Creates a child session with replayable history and the source's full retained task state. It uses the recorded workspace, independently inspects prior results, and continues unfinished work when no new prompt is supplied. A supplied prompt adds a request without dropping older work. Failure preserves the child and its diagnostics for another continuation.",
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
];
