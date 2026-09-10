export const toolPages = [
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
