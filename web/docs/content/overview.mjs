export const overviewPages = [
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
          "A task enters through CLI, TUI, RPC, ACP, headless or an SDK. Jeden records the original request before a model call and uses a separate read-only conversation to record its acceptance requirements. The execution loop proposes results; a fresh verifier reads real observations before the native controller accepts completion. A new question does not erase earlier work.",
          "Tool schemas are derived from each tool’s input contract and sent with the model request. Tool results are recorded in the session and returned to the model until it produces a final answer. File mutations return Jeden-native visual diffs and previews, and oversized tool results are persisted as session artifacts and replaced in the model loop with a compact reference.",
          "Failure handling is fail-closed. Without <code>BRAMA_URL</code> the run stops with <code>BRAMA_URL is required</code> and no model call is made. Transient model errors retry with the router’s backoff, but neither retry nor subscription failover happens once model output has become visible; a typed quota-exhaustion response records a <code>Retry-After</code>-bounded cooldown in <code>.jeden/subscription-cooldowns.json</code> before the next eligible subscription is selected.",
        ],
      },
      {
        title: "Quick start",
        paragraphs: [
          "Prerequisites: a supported platform (<code>aarch64-apple-darwin</code>, <code>x86_64-unknown-linux-gnu</code>, <code>x86_64-pc-windows-msvc</code>) or a Rust toolchain for source builds, a Brama-compatible model endpoint, and a caller-owned signing credential.",
          "Running <code>jeden</code> opens the welcome view. Its first screen can adopt an existing repository in place; the same operation is available as <code>jeden workspace adopt &lt;path&gt;</code>, <code>/setup workspace &lt;path&gt;</code>, and the Jeden Desktop Settings screen. The accepted canonical path becomes the default for the next task unless <code>--cwd</code> is explicit. <code>/setup</code> remains an idempotent wizard for workspace, Brama URL, agent id, default model, and preferences; it writes non-secret router values to <code>~/.jeden/.env</code> at mode <code>0600</code>, while workspace selection uses the atomic user config. <code>WISENT_APP_AGENT_AUTH_SECRET</code> is read from the process environment only — the harness holds no credential store and writes no secret to disk; the bundled launch scripts export it from the Skarbiec item <code>agent:wisent-app</code>, which also owns rotation and revocation. <code>jeden doctor</code> returns a JSON health report and exits non-zero when an active probe is unavailable.",
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
            code: 'jeden run "Respond exactly: OK" --model-only   # expected output: OK',
          },
        ],
      },
      {
        title: "Run a named VS Code task on macOS",
        paragraphs: [
          "On a configured Wisent workstation, <code>scripts/run-with-stado.sh</code> obtains the agent signing credential and the separate <code>jeden-model-router/token</code> bearer from Skarbiec, then launches the installed Jeden without building it. The reusable task in <code>scripts/vscode-tasks.json</code> runs a disk diagnosis with <code>gpt-6-astra</code> in a dedicated integrated terminal, without typing into another terminal's prompt or changing the default model.",
          "For a checkout at <code>~/Documents/CodingProjects/Wisent/jeden</code>, run the command below from the repository root only when VS Code has no user task file. If that file already exists, preserve it and add this task to its <code>tasks</code> array instead. Choose <strong>Terminal → Run Task… → Jeden: diagnoza dysku bez zmian</strong>; the named terminal retains the command and final exit status, and only one instance can run at a time.",
          "The task asks for paths, sizes, growth causes and APFS accounting without deletion, compilation, configuration changes or consent prompts. It grants command execution, not a filesystem sandbox; these restrictions are part of the diagnosis prompt. A Brama refusal remains a failed run instead of causing a model switch or a new login.",
        ],
        commands: [
          {
            label: "Install the source-backed user task",
            code: 'ln -s "$PWD/scripts/vscode-tasks.json" \\\n  "$HOME/Library/Application Support/Code/User/tasks.json"',
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
];
