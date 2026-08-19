/**
 * Jeden documentation content, shaped to the @wisent-ai/components DocPage
 * contract. Prose strings may carry the inline tags <code>, <strong>, <em>,
 * and <a href="…">; web/scripts/build-docs.mjs parses them into elements.
 *
 * This module is data only — the layout is the canonical DocumentationLayout
 * from @wisent-ai/components.
 */

export const product = "Jeden";
export const homeHref = "/";
export const sourceHref = "https://github.com/wisent-ai/jeden";
export const sourceLabel = "Harness source";

export const nav = [
  { label: "Overview", href: "/docs" },
  { label: "Sessions", href: "/docs/sessions" },
  { label: "Tools", href: "/docs/tools" },
];

export const pages = [
  {
    slug: "index",
    href: "/docs",
    file: "index.html",
    meta: {
      htmlTitle: "Overview — Jeden documentation",
      description:
        "Jeden documentation — a private coding-agent harness: local policy, controlled tools, durable sessions, and model freedom.",
      ogTitle: "Overview — Jeden documentation",
      ogDescription: "A private coding-agent harness built by Wisent.",
      canonical: "https://jeden.wisent.com/docs",
    },
    eyebrow: "The harness",
    title: "A coding agent you can <em>actually own.</em>",
    description:
      "Jeden is Wisent’s private coding-agent harness: local policy, controlled tools, durable sessions, and model freedom. One local process keeps the whole loop — from the first file read to the final verification — on your machine and under your rules.",
    sections: [
      {
        title: "What Jeden is",
        paragraphs: [
          "Jeden is a harness for AI agents built from real-life experiences. It routes models intelligently, manages credentials, and understands how to pursue, complete, and verify tasks over time — while the agent’s inference path, spend attribution, and tool permissions remain under your control rather than a third-party hosted agent’s. It is compatible with OpenAI, Anthropic, Kimi, and any other model reachable through an OpenAI-style endpoint.",
          "Jeden is not a hosted or multi-tenant service; it is a local harness, and its local runtime is usable without a hosted Wisent account. It runs as a single local process: nothing but that process reads the checkout, and inference is reachable only through the Brama model router — Jeden carries no provider API key and no provider SDK.",
          "Jeden serves two audiences:",
        ],
        bullets: [
          "<strong>Engineers</strong>, who run interactive and one-shot coding tasks in the terminal with local approval over every write and command.",
          "<strong>Tooling and automation</strong>, which drives the same harness through its machine interfaces (RPC, ACP, headless mode, SDKs) inside editors and workflows.",
        ],
      },
      {
        title: "Design contract",
        paragraphs: [
          "Jeden separates five concerns. Each part stays visible, inspectable, and local — the model can reason freely, but the harness decides what may actually happen.",
        ],
        bullets: [
          "<strong>Inference</strong> — model calls go through Brama using HMAC-signed, OpenAI-compatible chat completions: each request carries <code>x-agent-id</code>, <code>x-agent-timestamp</code>, <code>x-agent-body-sha256</code>, and <code>x-agent-signature</code>, so the signing secret itself never leaves the process.",
          "<strong>Policy</strong> — the harness prompt and approval rules are explicit and local.",
          "<strong>Tools</strong> — a small allowlisted registry enforces path jails and write or command permission.",
          "<strong>Run loop</strong> — the model may return native tool calls or strict JSON actions that enter the same local execution loop.",
          "<strong>Pursuit adapter</strong> — <code>jeden pursue</code> maps the separately owned Pursuit engine stages onto persistent planner and executor conversations plus fresh read-only reviewers.",
        ],
      },
      {
        title: "How a task runs",
        paragraphs: [
          "A task enters through one interface — CLI, TUI, <code>rpc</code>, <code>acp</code>, headless, or an SDK — and one run loop drives it to completion. The loop sends the conversation and the derived tool schemas to Brama, receives either a final answer or tool calls, executes each tool locally under the path jail and the approval policy, appends the outcome to the session ledger, and repeats until the model answers.",
          "Tool schemas are derived from each tool’s input contract and sent with the model request. Tool results are recorded in the session and returned to the model until it produces a final answer. File mutations return Jeden-native visual diffs and previews, and oversized tool results are persisted as session artifacts and replaced in the model loop with a compact reference.",
          "Failure handling is fail-closed. Without <code>BRAMA_URL</code> the run stops with <code>BRAMA_URL is required</code> and no model call is made. Transient model errors retry with the router’s backoff, but neither retry nor subscription failover happens once model output has become visible; a typed quota-exhaustion response records a <code>Retry-After</code>-bounded cooldown in <code>.jeden/subscription-cooldowns.json</code> before the next eligible subscription is selected.",
        ],
      },
      {
        title: "Quick start",
        paragraphs: [
          "Prerequisites: a supported platform (<code>aarch64-apple-darwin</code>, <code>x86_64-unknown-linux-gnu</code>, <code>x86_64-pc-windows-msvc</code>) or a Rust toolchain for source builds, a Brama-compatible model endpoint, and a caller-owned signing credential.",
          "Running <code>jeden</code> opens the welcome view; <code>/setup</code> is an idempotent wizard (Brama URL, agent id, default model, preferences) that writes only non-secret values to <code>~/.jeden/.env</code> at mode <code>0600</code>, and <code>/setup validate</code> probes live state and ends with a smoke call. <code>WISENT_APP_AGENT_AUTH_SECRET</code> is read from the process environment only — the harness holds no credential store and writes no secret to disk; the bundled launch scripts export it from the Skarbiec item <code>agent:wisent-app</code>, which also owns rotation and revocation. <code>jeden doctor</code> returns a JSON health report probing Brama, Weles, storage, process, MCP, extensions, LSP, browser, task, memory, collab, and keymap, and exits non-zero when any probe is unavailable. A successful setup is observable:",
        ],
        commands: [
          {
            label: "Build from source",
            code: "git clone https://github.com/wisent-ai/jeden.git && cd jeden\ncargo build --locked --release   # or: bin/jeden-rust",
          },
          {
            label: "Required environment for real model calls",
            code: "WISENT_APP_AGENT_AUTH_SECRET=<signing-credential>\nBRAMA_URL=<brama-model-router-url>\n# Set only when Brama requires its distinct bearer.\nBRAMA_TOKEN=<brama-bearer>\nWISENT_APP_AGENT_ID=wisent-app",
          },
          {
            label: "Verify",
            code: 'jeden run "Respond exactly: OK"   # expected output: OK',
          },
        ],
      },
      {
        title: "Current scope",
        paragraphs: ["The private milestone includes:"],
        bullets: [
          "interactive terminal and one-shot <code>jeden run</code> modes;",
          "autonomous outcome pursuit through <code>jeden pursue</code>, with source-grounded contracts, independent reviews, and durable receipts;",
          "session transcripts and artifacts under <code>~/.jeden/sessions/</code>;",
          "model routing through required <code>BRAMA_URL</code>, <code>WISENT_APP_AGENT_ID</code>, and <code>WISENT_APP_AGENT_AUTH_SECRET</code>;",
          "model selection through <code>--model</code>, <code>JEDEN_MODEL</code>, or native config;",
          "jailed filesystem, document, archive, image, SQLite, search, Git, process, evaluation, URL, artifact, memory, todo, delegation, and MCP tools;",
          "guarded file mutations using the digest or snapshot tag returned by <code>read_file</code>;",
          "custom JavaScript tools, project and user lifecycle hooks, and native <code>.jeden</code> configuration paths;",
          "transactional <code>jeden update</code> that verifies a DSSE release manifest against the binary’s embedded <code>canary</code> and <code>stable</code> ed25519 trust roots, checks the artifact digest plus SBOM and provenance evidence, and rolls back to the journaled last-known-good binary on failure;",
          "interactive approval for writes and commands unless explicitly enabled.",
        ],
        callout: {
          tone: "note",
          text: "Maturity: public development source at SemVer <code>0.x</code> — there is no stable public contract yet. Source is available under the Apache License 2.0; use the public <a href=\"https://github.com/wisent-ai/jeden/issues\">wisent-ai/jeden issue tracker</a> for non-sensitive reports and GitHub Security Advisories for vulnerabilities.",
        },
      },
    ],
  },
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
        title: "Inspect, export, resume",
        paragraphs: [
          "<code>jeden export</code>, <code>show</code>, <code>artifacts</code>, <code>artifact</code>, <code>search-sessions</code>, <code>resume</code>, and <code>recall_conversation</code> inspect or reuse recorded work. Resumed work starts a fresh session seeded with the selected history; abandoned history is never deleted.",
        ],
        commands: [
          {
            label: "Session commands",
            code: 'jeden sessions\njeden show <session>\njeden export <session> <output>\njeden artifacts <session>\njeden artifact <session> <name> <output>\njeden resume <session> "continue"\njeden search-sessions "query"\njeden recall_conversation --list',
          },
        ],
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
    ],
  },
  {
    slug: "tools",
    href: "/docs/tools",
    file: "tools.html",
    meta: {
      htmlTitle: "Tools — Jeden documentation",
      description:
        "Jeden tools — an allowlisted, jailed tool registry with approval-gated writes and commands, guarded mutations, custom tools, MCP, and hooks.",
      ogTitle: "Tools — Jeden documentation",
      ogDescription: "Controlled tools and interfaces in the Jeden harness.",
      canonical: "https://jeden.wisent.com/docs/tools",
    },
    eyebrow: "Controlled execution",
    title: "Controlled tools, <em>accountable effects.</em>",
    description:
      "A small allowlisted registry enforces path jails and write or command permission. Tool schemas are derived from each input contract and sent with the model request; every result is recorded in the session and returned to the model until it produces a final answer.",
    sections: [
      {
        title: "The tool registry",
        paragraphs: [
          "The current scope ships jailed filesystem, document, archive, image, SQLite, search, Git, process, evaluation, URL, artifact, memory, todo, delegation, and MCP tools. File mutations return Jeden-native visual diffs and previews; oversized tool results are persisted as session artifacts and replaced in the model loop with a compact reference.",
          "The harness ships an explicit tool policy for the model:",
        ],
        bullets: [
          "discover files with <code>glob_paths</code> or <code>list_dir</code> before reading unknown paths;",
          "search content with <code>grep_regex</code> or <code>search_files</code> rather than shell discovery commands;",
          "use targeted <code>read_file</code> selectors instead of dumping large files;",
          "use <code>run_package_script</code> for declared package scripts and reserve general process tools for commands without a safer built-in;",
          "verify behavior changes with the narrowest relevant check.",
        ],
      },
      {
        title: "Approvals and grants",
        paragraphs: [
          "Every file write or shell command pauses for interactive approval unless explicitly enabled, and destructive confirmations default to <strong>Cancel</strong>. In one-shot mode, grants are explicit per invocation. Project hooks in <code>.jeden/hooks.json</code> run only with <code>--allow-command</code>, so a cloned repository cannot silently execute shell.",
        ],
        commands: [
          {
            label: "One-shot grants",
            code: 'jeden run "summarize package.json"\njeden run "create notes.txt" --allow-write\njeden run "inspect the build" --allow-command',
          },
        ],
      },
      {
        title: "Guarded mutations",
        paragraphs: [
          "File mutations are guarded: edits use the digest or snapshot tag returned by <code>read_file</code>, and snapshot-tagged edits reject stale state instead of overwriting it.",
        ],
      },
      {
        title: "Network access",
        paragraphs: [
          "Jeden initiates every connection. The terminal, <code>jeden run</code>, <code>jeden rpc</code>, and <code>jeden acp</code> are stdio-only and open no socket; listening sockets exist only in the opt-in <code>jeden headless <addr></code> (mutual TLS), <code>jeden collab-relay</code>, and <code>jeden stats --serve</code> (bound to <code>127.0.0.1</code>).",
          "The one required outbound dependency is <code>BRAMA_URL</code>; optional dependencies — Wisent Platform Billing for subscription and quota decisions, the Stado integration and media APIs, and the release manifest host for <code>jeden update</code> — activate only when configured. Tool-initiated network access (<code>fetch_url</code>, <code>fetch_readable_url</code>, SSH) is checked against the execution grant’s host and port allowlist with pinned addresses and re-authorized redirects.",
        ],
      },
      {
        title: "Custom tools, MCP, and hooks",
        paragraphs: [
          "Custom JavaScript tools load from <code>~/.jeden/tools/</code> and <code><cwd>/.jeden/tools/</code>. A custom module exports a default factory that receives the current workspace helpers and returns one tool or a list of tools; tool names must be unique and cannot collide with built-ins. Custom execution remains subject to the same jail, approval, and hook policy as built-in tools.",
          "MCP servers load from <code>~/.jeden/mcp.json</code> and <code><cwd>/.jeden/mcp.json</code> using the standard <code>mcpServers</code> shape. Generic MCP tools list and call server tools, resources, and prompts; configured server tools may also appear under native <code>mcp__<server>__<tool></code> names.",
          "Shared lifecycle hooks receive user-prompt, pre-tool, post-tool, session-start, and stop events. Hook output may add context, replace supported input fields, or block an action through the documented decision contract.",
        ],
        commands: [
          {
            label: "List active tools",
            code: "jeden tools --cwd .",
          },
        ],
      },
      {
        title: "Interfaces",
        bullets: [
          "<strong>CLI</strong> (<code>jeden</code>, <code>jeden run</code>, <code>jeden pursue</code>, management subcommands) — canonical for human interactive, direct one-shot, and contract-driven autonomous use.",
          "<strong>Interactive terminal views and slash commands</strong> — canonical for in-terminal management; non-terminal stdin renders deterministic text lists for scripts.",
          "<strong><code>jeden rpc</code> (NDJSON), <code>jeden acp</code>, <code>jeden headless</code></strong> — canonical for automation and editor integration; <code>--json</code> flags cover scripting.",
          "<strong>SDKs</strong> — <code>packages/sdk-typescript</code> and <code>python/jeden_sdk</code> for embedding the machine interfaces.",
          "<strong>MCP</strong> — the extension interface for external tool servers.",
        ],
        callout: {
          tone: "note",
          text: "In a terminal, management commands without arguments open native searchable views instead of printing command syntax; selecting a row dispatches the same validated slash command that can still be entered directly. Use <code>run</code> when the supplied task is already concrete; use <code>pursue</code> when the input is only an intent seed and Pursuit must recover the concrete outcome, boundaries, preferences, evidence, and finish line first.",
        },
      },
    ],
  },
];
