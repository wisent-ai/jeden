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
          "Failure handling is fail-closed. Without <code>BRAMA_URL</code> the run stops with <code>BRAMA_URL is required</code> and no model call is made. A transient model error moves the request to the next eligible subscription or route at once, with no backoff sleep, and the last error is what the run reports when none is left; no failover happens once model output has become visible. A typed quota-exhaustion response records a <code>Retry-After</code>-bounded cooldown in <code>.jeden/subscription-cooldowns.json</code> before the next eligible subscription is selected. <code>modelRouting.retry.baseDelayMs</code>, <code>maxDelayMs</code> and <code>jitterRatio</code> were removed.",
          "Nothing waiting is cut short. A model that takes minutes to send its first token, a tool process that runs for an hour, an MCP server that is building something, a language server indexing a repository and an editor an operator is typing in all run until they finish; an unreachable gateway still fails at once, because a refused connection is an answer. What ends work early is the operator cancelling the turn, and every wait watches for that. There is no <code>timeoutMs</code> on a tool, no <code>modelRouting.retry.firstEventTimeoutMs</code> and no <code>idleTimeoutMs</code>: those settings were removed rather than re-tuned.",
          "The terminal redraws when keyboard input, a resize, worker output or worker completion arrives, not on a polling timer. Approval questions use that same event source. The busy indicator advances with observed activity; a quiet worker does not animate it. Foreground commands and external editors receive stdin without a competing reader.",
          "Model discovery is not a gate either. A catalog read that fails, is rate limited or answers 5xx leaves the catalog unread rather than answering about the configured model, so the run says so on standard error and sends the request anyway; the gateway that serves it decides. A catalog that does answer still refuses an unknown or unavailable model by name, and an explicit non-retryable refusal still stops the run before any provider spend.",
          "An answer the model does deliver can still be unusable: cut off mid-JSON, or stopped by the output budget. That is one answer failing, not the work failing, so the turn quotes the exact refusal back to the model once and only then ends — naming what happened to the answer, with the retained request still open.",
          "Native intake and acceptance inspectors accept their structured JSON directly, inside a Markdown code block, or encoded in the text of a final action. Read-only tool actions still run through the same permission checks. A parsed answer is not evidence of completion: the controller still checks every acceptance requirement and its observation receipts.",
        ],
      },
      {
        title: "Quick start",
        paragraphs: [
          "Prerequisites: a supported platform (<code>aarch64-apple-darwin</code>, <code>x86_64-unknown-linux-gnu</code>, <code>x86_64-pc-windows-msvc</code>) or a Rust toolchain for source builds, a Brama-compatible model endpoint, and a caller-owned signing credential.",
          "Running <code>jeden</code> opens the welcome view. Its first screen can adopt an existing repository in place; the same operation is available as <code>jeden workspace adopt &lt;path&gt;</code>, <code>/setup workspace &lt;path&gt;</code>, and the Jeden Desktop Settings screen. The accepted canonical path becomes the default for the next task unless <code>--cwd</code> is explicit. <code>/setup</code> remains an idempotent wizard for workspace, Brama URL, agent id, default model, and preferences; it writes non-secret router values to <code>~/.jeden/.env</code> at mode <code>0600</code>, while workspace selection uses the atomic user config. <code>WISENT_APP_AGENT_AUTH_SECRET</code> is read from the process environment only — the harness holds no credential store and writes no secret to disk. The bundled launch scripts obtain the signing credential through Stado; Skarbiec owns its rotation and revocation. <code>jeden doctor</code> returns a JSON health report and exits non-zero when an active probe is unavailable.",
          "On a Stado host Jeden asks Stado where Brama is at every start: <code>stado service directory connect brama --consumer jeden --no-verify --json</code> answers with the loopback adapter this host's resolver binds for the consumer <code>jeden</code>, and that address becomes <code>BRAMA_URL</code> for the process. Only a <code>BRAMA_URL</code> exported in the caller's environment overrides it; an address in <code>~/.jeden/.env</code> is used only when Stado cannot answer, and then Stado's refusal is named. The adapter port is handed out by the host and changes when it is reassigned, which is why a written copy goes stale. The route is declared once per host with <code>stado service directory consumer-add brama jeden --capability model-routing --target &lt;host&gt; --json</code>; the gateway bearer is the item playing role <code>jeden-model-router</code>. <code>jeden doctor</code>, <code>/setup</code> and <code>jeden token</code> say where the address came from, and a stale file copy that the route replaced is named.",
          "On macOS, source builds also need Stado 0.22.0 or later and an available Apple Development or Developer ID Application identity. Build <code>jeden</code> and <code>jeden-sandbox-helper</code> together, then run <code>stado product signing sign --product jeden target/release/jeden target/release/jeden-sandbox-helper</code>. The helper must stay beside the executable. Signing failures stop installation; there is no ad-hoc fallback. The shared contract is <a href=\"https://stado.wisent.com/docs/signing\">Native macOS code signatures</a>.",
          "Stado publishes Jeden as a command-line package, not a running fleet service. Its Darwin archive carries <code>bin/jeden</code> and <code>bin/jeden-sandbox-helper</code>; consumers must install both from the same archive and keep them beside each other. Publication alone is not proof that a consumer can run a task.",
          "Delivery hosts belong to Stado's canonical registry, not Jeden's source. Each native delivery selects <code>target: { &quot;product&quot;: &quot;jeden&quot; }</code>; Stado expands the CLI and sandbox-helper deliveries onto every declared target of the matching platform. Read the declaration with <code>stado release destinations show jeden --json</code>. When the catalog still holds the former explicit deliveries, enrollment adopts their exact host set only if every installation operation is preserved; it refuses changed target coverage or dropped operations.",
          "Stado freezes the selected destinations for each release run. A later registry edit does not move a resumed delivery or redelivery. Both the submitter and workers must support object-valued delivery targets before releasing this manifest; an older reader refuses it rather than installing on an arbitrary builder. The <a href=\"https://stado.wisent.com/docs/delivery-destinations\">delivery destination contract</a> covers declaration changes, migration, retained placement and refusal diagnostics.",
          "The release worker signs the declared native stage before creating the archive. The Darwin recipe supplies the signing certificate and private key through scoped workload credentials and uses the signer's temporary keychain, without a system consent dialog. A missing credential, unusable signing identity or missing helper remains a failed build or run. See <a href=\"https://stado.wisent.com/docs/release\">Stado release and compatibility</a> for the source-bound publication commands and receipts.",
          "Release builders receive private Git dependencies through the immutable <code>private-cargo-sources</code> input, not through GitHub credentials or sibling checkouts. After changing the private package names, versions or revisions in a committed checkout, run <code>stado release catalog pin-input . --name private-cargo-sources --source . --revision HEAD --cargo --json</code> and commit the updated <code>.wisent-release.json</code>. Stado vendors, publishes and pins the input. Changes only to Jeden's own version keep it valid. The shared <a href=\"https://stado.wisent.com/docs/builds#private-cargo-build-inputs\">private Cargo input contract</a> covers publication, desktop controls and refusals.",
          "Quality uses <code>stado product cargo build</code>. The native build step is <code>stado product cargo stage --bin jeden --bin jeden-sandbox-helper</code>: the same Stado Cargo executor builds each binary from the locked sources and places it at <code>WISENT_OUTPUT_DIR/&lt;name&gt;</code>, and the recipe archives them as <code>bin/jeden</code> and <code>bin/jeden-sandbox-helper</code>. An absent output directory setting is refused before native compilation. Jeden carries no staging, private-source export or source-replacement code of its own.",
          "The worker's Stado checks the mounted input against the private packages in <code>Cargo.lock</code> and checks its source configuration against its provenance before Cargo starts. Missing input, mismatched packages and unsupported provenance are refusals. Git transport is disabled; Cargo verifies the exported files and preserves the lockfile. Public registry packages still use normal Cargo resolution.",
          "The shared publication journey is <code>tests/release/private-cargo-sources.mjs</code> in Stado. It uses a real committed Cargo source, publishes and reads an immutable archive through isolated local storage, then runs Cargo metadata with mounted sources and no private Git cache. Its records include the source and binary identity, command output, exit statuses and persisted manifest. It does not compile or qualify a native application.",
          "Each native Jeden release runs <code>JEDEN_TEST_SUCCESS_EXIT=0 node tests/release/stage.mjs</code> with the worker's source, source commit, output and private-input environment. It builds and executes the staged CLI, records the helper's staged bytes, and checks missing-output and missing-input refusals without replacing successful artifacts or changing the lockfile. Reports are archived under <code>evidence/release-tests</code>. This is not proof of signing, sandbox execution or a model-backed task.",
        ],
        commands: [
          {
            label: "Build from source",
            code: "git clone https://github.com/wisent-ai/jeden.git && cd jeden\ncargo build --locked --release\n# macOS only, after building:\nstado product signing sign --product jeden target/release/jeden target/release/jeden-sandbox-helper",
          },
          {
            label: "Environment for real model calls without Stado",
            code: "WISENT_APP_AGENT_AUTH_SECRET=<signing-credential>\nBRAMA_URL=<brama-model-router-url>\n# Set only when Brama requires its distinct bearer.\nBRAMA_TOKEN=<brama-bearer>\nWISENT_APP_AGENT_ID=wisent-app",
          },
          {
            label: "Verify",
            code: 'jeden run "Respond exactly: OK" --model-only   # expected output: OK',
          },
        ],
      },
      {
        title: "Run without Brama or Stado",
        paragraphs: [
          "A user without Brama points Jeden at any OpenAI-compatible provider with <code>JEDEN_MODEL_ENDPOINT</code>, for example <code>https://api.openai.com</code> or a local <code>http://127.0.0.1:11434</code>. Jeden then reads the model list from that provider's <code>/v1/models</code> and sends chat to its <code>/v1/chat/completions</code>, with <code>JEDEN_MODEL_KEY</code> as the bearer when the provider needs one. It does not ask Stado for the Brama credentials and does not sign requests.",
          "Only this setting turns Brama off. Without it, a missing Brama is still a refusal: <code>BRAMA_URL is required; declare Jeden's route with `stado service directory consumer add brama jeden --capability model-routing --target &lt;this host&gt;`, export BRAMA_URL, or set JEDEN_MODEL_ENDPOINT to an OpenAI-compatible provider to run without Brama</code>, followed by <code>(Stado did not route it: …)</code> with Stado's own sentence when Stado was asked, and a Brama run without its signing credential stops with <code>WISENT_APP_AGENT_AUTH_SECRET is required to sign requests to Brama</code>. A provider's model list carries no fallbacks, promotions or prices, so only the fallbacks in <code>modelRouting</code> apply and usage is recorded without cost.",
          "<code>/rebuild</code> on macOS asks Stado for the running code identity and has Stado sign the new build. Without Stado it stops with <code>cannot inspect the running code identity through Stado: …; without Stado set JEDEN_CODESIGN_IDENTITY to a codesign identity, or - for ad-hoc</code>. With <code>JEDEN_CODESIGN_IDENTITY</code> set it signs the new build with <code>/usr/bin/codesign --force --sign &lt;identity&gt;</code>, asks Stado nothing, and does not compare that identity with the running one.",
        ],
        commands: [
          {
            label: "Environment for a direct provider",
            code: 'JEDEN_MODEL_ENDPOINT=https://api.openai.com\nJEDEN_MODEL_KEY=<provider-api-key>\njeden run "Respond exactly: OK" --model gpt-4o --model-only',
          },
        ],
      },
      {
        title: "Run a named VS Code task on macOS",
        paragraphs: [
          "On a configured Wisent workstation the installed Jeden reads its agent signing credential and distinct model-router bearer through <code>stado credentials get</code>, under Stado's identity. The reusable task in <code>scripts/vscode-tasks.json</code> runs a disk diagnosis with <code>gpt-6-astra</code> in a dedicated integrated terminal, without typing into another terminal's prompt or changing the default model.",
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
          "model routing through <code>BRAMA_URL</code>, <code>WISENT_APP_AGENT_ID</code>, and <code>WISENT_APP_AGENT_AUTH_SECRET</code>, or through an OpenAI-compatible provider named by <code>JEDEN_MODEL_ENDPOINT</code>;",
          "model selection through <code>--model</code>, <code>JEDEN_MODEL</code>, or native config;",
          "goal-lifecycle classification of each prompt by the lifecycle model Brama serves under the alias <code>JEDEN_LIFECYCLE_MODEL_ALIAS</code> names, signed like every other model call; without the alias there is no classifier, and a refused call is recorded in the session ledger as <code>goal_lifecycle_refused</code> with Brama's reason;",
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
