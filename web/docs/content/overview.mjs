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
          "Nothing waiting is cut short. A model that takes minutes to send its first token, a tool process that runs for an hour, an MCP server that is building something, a language server indexing a repository and an editor an operator is typing in all run until they finish; an unreachable gateway still fails at once, because a refused connection is an answer. What ends work early is the operator cancelling the turn, and every wait watches for that. There is no <code>timeoutMs</code> on a tool, no <code>modelRouting.retry.firstEventTimeoutMs</code> and no <code>idleTimeoutMs</code>: those settings were removed rather than re-tuned.",
          "Model discovery is not a gate either. A catalog read that fails, is rate limited or answers 5xx leaves the catalog unread rather than answering about the configured model, so the run says so on standard error and sends the request anyway; the gateway that serves it decides. A catalog that does answer still refuses an unknown or unavailable model by name, and an explicit non-retryable refusal still stops the run before any provider spend.",
          "An answer the model does deliver can still be unusable: cut off mid-JSON, or stopped by the output budget. That is one answer failing, not the work failing, so the turn quotes the exact refusal back to the model once and only then ends — naming what happened to the answer, with the retained request still open.",
          "Native intake and acceptance inspectors accept their structured JSON directly, inside a Markdown code block, or encoded in the text of a final action. Read-only tool actions still run through the same permission checks. A parsed answer is not evidence of completion: the controller still checks every acceptance requirement and its observation receipts.",
        ],
      },
      {
        title: "Quick start",
        paragraphs: [
          "Prerequisites: a supported platform (<code>aarch64-apple-darwin</code>, <code>x86_64-unknown-linux-gnu</code>, <code>x86_64-pc-windows-msvc</code>) or a Rust toolchain for source builds, a Brama-compatible model endpoint, and a caller-owned signing credential.",
          "Running <code>jeden</code> opens the welcome view. Its first screen can adopt an existing repository in place; the same operation is available as <code>jeden workspace adopt &lt;path&gt;</code>, <code>/setup workspace &lt;path&gt;</code>, and the Jeden Desktop Settings screen. The accepted canonical path becomes the default for the next task unless <code>--cwd</code> is explicit. <code>/setup</code> remains an idempotent wizard for workspace, Brama URL, agent id, default model, and preferences; it writes non-secret router values to <code>~/.jeden/.env</code> at mode <code>0600</code>, while workspace selection uses the atomic user config. <code>WISENT_APP_AGENT_AUTH_SECRET</code> is read from the process environment only — the harness holds no credential store and writes no secret to disk; the bundled launch scripts export it from the Skarbiec item <code>agent:wisent-app</code>, which also owns rotation and revocation. <code>jeden doctor</code> returns a JSON health report and exits non-zero when an active probe is unavailable.",
          "On macOS, source builds also need Wisent Products and an available Apple Development or Developer ID Application identity. Build <code>jeden</code> and <code>jeden-sandbox-helper</code> together, then run <code>wisent-products signing sign --product jeden target/release/jeden target/release/jeden-sandbox-helper</code>. The helper must stay beside the executable. Signing failures stop installation; there is no ad-hoc fallback. The shared contract is <a href=\"https://stado.wisent.com/docs/signing\">Native macOS code signatures</a>.",
          "Stado publishes Jeden as a command-line package, not a running fleet service. Its Darwin archive carries <code>bin/jeden</code> and <code>bin/jeden-sandbox-helper</code>; consumers must install both from the same archive and keep them beside each other. Publication alone is not proof that a consumer can run a task.",
          "The release worker signs the declared native stage before creating the archive. The Darwin recipe supplies <code>desktop-signing-apple-development#certificate</code> and <code>desktop-signing-apple-development#private_key</code> through scoped workload credentials and uses the signer's temporary keychain, without a system consent dialog. A missing credential, unusable signing identity or missing helper remains a failed build or run. See <a href=\"https://stado.wisent.com/docs/release\">Stado release and compatibility</a> for the source-bound publication commands and receipts.",
          "Release builders receive private Git dependencies through the immutable <code>private-cargo-sources</code> input, not through GitHub credentials or sibling checkouts. After a change to the private Git packages <code>Cargo.lock</code> locks (a new revision, version or crate), run <code>cargo run --locked --manifest-path tools/Cargo.toml -- release export .wisent-output/private-cargo-sources.tar.gz</code>, publish the returned archive with <code>stado storage put &lt;input.uri&gt; &lt;archive&gt; --if-absent</code>, and record its returned input object in <code>.wisent-release.json</code>. Any other lockfile change, such as jeden's own version, keeps the published input valid. The export refuses an archive path outside <code>.wisent-output</code> and a lockfile that changes during the export. The release Cargo wrapper refuses missing inputs and inputs whose private packages (name, version, source) differ from the ones <code>Cargo.lock</code> locks, listing both sets in <code>private Cargo sources do not match Cargo.lock; export and publish a new input (the input carries …, Cargo.lock locks …)</code>; Cargo verifies the exported checksums without changing the lockfile.",
          "The release tooling is the Rust package in <code>tools/</code>, its own Cargo workspace so it builds before the private sources are configured. Quality runs <code>cargo run --locked --manifest-path tools/Cargo.toml -- release cargo ...</code> and building runs <code>... -- release stage --bin jeden</code>, adding <code>--bin jeden-sandbox-helper</code> and <code>--qualify pursuit</code>. Staging names the tool it runs as and copies only successful native build outputs from the source's <code>target</code> directory into <code>WISENT_OUTPUT_DIR/bin</code>; each <code>--qualify TEST</code> then runs that integration test's ignored journeys against the staged candidate, and a failing journey fails the build. The journeys live in the build step because Stado 0.21.48, still installed on the Linux builder, refuses a recipe <code>tests</code> key with <code>unknown recipe keys for this Stado: tests</code>. An absent output directory setting is refused with <code>WISENT_OUTPUT_DIR is required for native staging</code> before compilation.",
          "The wrapper finds Cargo on <code>PATH</code>, then in <code>$CARGO_HOME/bin</code>, else <code>$HOME/.cargo/bin</code>. It preserves the proxy name used by Rustup and does not require shell startup files. If no executable exists, the refusal names the missing path and asks for toolchain provisioning; the wrapper does not install tools or change host configuration.",
          "Run <code>cargo test --target-dir target/qualification --test release -- --ignored export_and_stage_offline</code> on a development host with read access to the private crates to exercise the export, offline source selection with a service-style <code>PATH</code>, actual helper staging and execution, and missing-output, missing-toolchain, missing-input, lockfile-mismatch and missing-package refusals. The separate target directory keeps the nested staging build from waiting on the test's own build lock. Reports under <code>.wisent-output/release-tests/</code> retain the source revision, patch, command output, exit codes and staged helper hash. This check does not qualify a signed sandbox or a model-backed task.",
          "Each native release also runs the <code>stage_from_declared_input</code> journey of <code>tests/release</code> through the release Cargo wrapper, with <code>--target-dir target/qualification</code> so its nested staging build does not wait on the test's own build lock. It builds and executes the staged Jeden CLI and checks staging refusals without exporting private sources or needing GitHub credentials, records the worker's <code>WISENT_SOURCE_COMMIT</code>, and the archive retains its reports under <code>evidence/release-tests/</code>.",
        ],
        commands: [
          {
            label: "Build from source",
            code: "git clone https://github.com/wisent-ai/jeden.git && cd jeden\ncargo build --locked --release\n# macOS only, after building:\nwisent-products signing sign --product jeden target/release/jeden target/release/jeden-sandbox-helper",
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
