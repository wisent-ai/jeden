export const contextPages = [
  {
    slug: "context",
    href: "/docs/context",
    file: "context.html",
    meta: {
      htmlTitle: "Context — Jeden documentation",
      description:
        "Jeden's context advisor — what an agent should read before it searches, from every readable file, its own memory, the transcript archive and the Wisent ground-truth index, with every source's state reported.",
      ogTitle: "Context — Jeden documentation",
      ogDescription:
        "Ranked context recommendations with exact locators, injected into every turn and callable from the CLI, the agent tool set and Jeden Desktop.",
      canonical: "https://jeden.wisent.com/docs/context",
    },
    eyebrow: "Context management",
    title: "Read the right thing <em>first.</em>",
    description:
      "The context advisor answers one question — what should be read for this task — and answers it with locators: a file and its line range, a session id, a cited repository path. Every turn starts with that answer, and every source says whether it could answer at all.",
    sections: [
      {
        title: "What arrives without being asked for",
        paragraphs: [
          "Jeden already loads what is always true: the discovered context files (<code>JEDEN.md</code>, <code>AGENTS.md</code>, <code>CLAUDE.md</code>, <code>RULES.md</code>, <code>~/.jeden/instructions.md</code>, <code>~/.jeden/context.md</code>) and the memories recorded for the working directory. Neither answers what is relevant to the request that just arrived.",
          "The advisor answers that second question in the turn prologue. Before the first model call, the task text is matched against the configured sources and the result is appended to the turn as a <code>[Context recommendations]</code> block: one line per recommendation with its source, its exact locator, its heading and a snippet, followed by every source that answered nothing and why. The block is recorded in the session's own <code>user</code> event, so what the model received is auditable afterwards.",
          "The block is bounded by <code>context.advisor.maxChars</code> and carries the sentence <em>ranked matches, not verified answers</em>: a recommendation is a place to look, never a fact. Set <code>context.advisor.enabled</code> to false to stop injecting it; <code>jeden context prompt \"&lt;task&gt;\"</code> prints exactly what a turn would receive, and says so when the advisor is off.",
        ],
        commands: [
          {
            label: "See what a turn receives",
            code: 'jeden context prompt "how is a release manifest signed"\njeden config set context.advisor.enabled false\njeden context prompt "how is a release manifest signed"',
          },
        ],
      },
      {
        title: "The four sources, and who owns each",
        paragraphs: [
          "<strong>files</strong> reads everything readable under the declared roots — documentation, source code, configuration, manifests — and ranks it in chunks. Markdown is cut at its headings, because that is where its meaning starts; every other file is cut where a top-level opener starts — a line at column zero after a blank line — and titled by that line, which in source is the declaration the rest of the chunk belongs to and in prose is a paragraph's opening. The locator is <code>path:first-last</code>, so the answer can be read directly, and the snippet is every line of the chunk that carries a query word, whole. Roots are declared in <code>context.advisor.roots</code> as colon-separated <code>path</code> or <code>path@depth</code> entries and default to the project plus <code>~/.jeden</code>; a root without a declared depth is walked whole. A file with a zero byte in it is not text and is skipped, and <code>.gitignore</code> is respected; no file is skipped for its size and no walk stops at a file or chunk count.",
          "On macOS a walk rooted at the home folder reads only the home's own files and enters none of the folders directly under it. Those folders hold the user's own things — photos, music, the desktop, downloads, other apps' data — and macOS guards them with a privacy prompt in the name of whichever process started Jeden (Oko, when Oko's broker resumed the session), so a session whose working directory is the home asked for Apple Music, Photos, Desktop and Downloads at every turn. A folder under the home is walked when it is itself a declared root, as <code>~/agents@1</code> is in the example below. The search tools (<code>grep</code>, <code>glob</code>) apply the same rule to a search rooted at the home.",
          "<strong>memory</strong> recalls what earlier sessions wrote down for this working directory. The store and its ranking belong to Jeden's memory subsystem — the fleet database <code>jeden</code>, scope <code>repo:&lt;cwd&gt;</code> — and the locator is <code>memory:&lt;id&gt;</code>. An empty store is reported as an empty store, not as a missing match.",
          "<strong>transcripts</strong> asks Transcript Lake, which owns the masked canonical archive of every recorded agent session. The advisor runs that product's own <code>search</code> command rather than reading its files, so the masking stays applied; the locator is <code>session:&lt;id&gt;</code>, which <code>jeden show</code> and the <code>recall_conversation</code> tool read.",
          "<strong>ground-truth</strong> asks the Wisent ground-truth index, which owns cited answers across the organization's repositories. The locator is the citation it returns — <code>repo/path@commit:first-last</code>. The endpoint comes from <code>context.advisor.groundTruthUrl</code>, or from <code>WISENT_GROUND_TRUTH_API</code> or <code>GROUND_TRUTH_API</code> when that is empty; <code>jeden context sources</code> reports which of the three it used.",
        ],
      },
      {
        title: "Every source answers, and each one runs to completion",
        paragraphs: [
          "<code>context.advisor.sources</code> defaults to <code>files,memory</code>. Files are local; memory opens the fleet database through Stado and Skarbiec. Sources run concurrently, so a turn waits for the slowest rather than for their sum. <code>--source all</code> also consults the archive and the ground-truth index.",
          "Nothing cuts a source short. There is no deadline setting and no <code>--timeout-ms</code> flag. A selected source that is waiting on a dependency holds the answer until that dependency responds or fails; this is not model reasoning. Stado's shared wait diagnostics on standard error identify a database operation or child command before waiting and report its outcome. Standard output remains the recommendation JSON. A failed source retains the dependency's error, not just its first progress line.",
          "Each selected source emits its own <code>czekam</code> start and completion or failure line, naming the source and workspace, in addition to dependency-level waits. A source thread that panics keeps its source name and panic cause in the unavailable result instead of becoming an unknown source. These diagnostics do not impose a deadline or silently omit a selected source.",
          "An unknown source name is refused rather than dropped: <code>unknown source(s): nonsense. Known sources: files, ground-truth, memory, transcripts</code>. A narrowed answer is never the result of a typo.",
        ],
        commands: [
          {
            label: "Select the sources to consult",
            code: 'jeden context recommend "why did the release agent quarantine that host" --limit 8 --source all\njeden context recommend "why did the release agent quarantine that host" --limit 8 --source files,memory',
          },
        ],
      },
      {
        title: "Reading a short list",
        paragraphs: [
          "Every answer carries a state for every source it consulted: <code>available</code> or <code>unavailable</code>, the observed reason, how much was considered, how much was returned, and how long it took. A list that is short because a source refused therefore looks different from a list that is short because nothing matched.",
          "When recommendations are empty and any source was unavailable, the terminal and prompt report that sources could not answer; they do not claim that no matching material exists. Only a completed search across available sources can establish an empty match list.",
          "JSON recommendations expose <code>unanswered</code>: the full source diagnostics for each consulted source that could not provide a valid answer. Successful searches with no matching material are not in this list. CLI, RPC and the agent tool serialize this from the same source outcomes; it is not a fabricated empty fallback. This is a final-result field, not a claim that a still-running dependency has already stopped.",
          "The files source reads every root it is given to the end, so its count of files and chunks is the whole corpus under the declared roots and depths.",
          "An empty memory recall is already a completed answer: recommendations do not launch an additional whole-store health query merely to describe that empty result. The explicit sources probe still reads health; a reachable store with no active records is available, while a missing or invalid health count is reported as an invalid reply rather than silently treated as zero.",
          "A ground-truth search reply must contain its results array. A successful HTTP status with a missing or wrongly typed results field is an unavailable source with the endpoint and schema failure, not a successful empty search.",
          "Each returned ground-truth recommendation also requires the index's nonempty <code>repo</code>, <code>path</code> and <code>commitSha</code>, positive ordered <code>lineStart</code>/<code>lineEnd</code>, string <code>content</code> and numeric <code>score</code>. A malformed selected result makes this source unavailable, naming its URL, HTTP status, result index and decoding or range error. No partial recommendations from that source or invented unknown citation are returned. Titles name the path and append the index's optional heading; a branch name does not replace a missing commit.",
          "<code>jeden context sources</code> asks the same question with no query: which roots exist and how many chunks they hold, how many active memories the store carries, which runtime partitions the archive holds, and whether the ground-truth endpoint answers its health route. A configured endpoint nobody serves reports the URL it could not reach, not silence.",
        ],
        commands: [
          { label: "The state of every source", code: "jeden context sources\njeden context sources --json" },
        ],
      },
      {
        title: "How a recommendation is ranked",
        paragraphs: [
          "The files source weighs each query word by how rare it is in the corpus it just read, so a word that occurs in nearly every chunk contributes nearly nothing and no list of words to ignore exists anywhere in the product. A hit in a heading, a declaration or a path outweighs a hit in a body; covering more of the query outweighs repeating one word; a long chunk is damped so length alone cannot win.",
          "A word also matches its own stem, so <code>routingu</code> finds <code>routing</code> and <code>signing</code> finds <code>signed</code>, at half the weight of an exact hit. Matching is lexical: a question asked in one language does not reach a document written in another unless they share words, and product names usually are those shared words.",
          "The other three sources rank with their own engines — the memory store's index, Transcript Lake's query, the ground-truth index's score — because each owns its own corpus. Scores are therefore comparable inside a source and not across sources, so the answer interleaves them in a fixed order instead of sorting one list by number.",
        ],
      },
      {
        title: "Every surface",
        paragraphs: [
          "<strong>CLI:</strong> <code>jeden context recommend</code>, <code>prompt</code>, <code>sources</code>. A bare first word is the task, so <code>jeden context \"why does signing fail\"</code> works.",
          "<strong>Interactive:</strong> <code>/context &lt;task&gt;</code> renders the same recommendations; bare <code>/context</code> keeps reporting the live window size.",
          "<strong>Agent tool:</strong> <code>context_recommend</code> takes <code>query</code>, and optionally <code>limit</code>, <code>sources</code>. It is a read-tier tool, because it does what the prologue already does unapproved.",
          "<strong>RPC and Jeden Desktop:</strong> <code>context/recommend</code> and <code>context/sources</code> answer the same objects the CLI prints with <code>--json</code>. Desktop's Context screen is that RPC, not a parsed command line. A request's parameters are read as stated, never replaced: a <code>query</code> that is absent or blank is refused with <code>context/recommend requires a query</code>, one that is not a string with <code>context/recommend query must be a string; got &lt;value&gt;</code>, a <code>limit</code> that is not a whole number with <code>context/recommend limit must be a whole number; got &lt;value&gt;</code>, and a <code>cwd</code> that is not a string with <code>context cwd must be a path string; got &lt;value&gt;</code>, each as <code>invalid_params</code>. A request with no <code>cwd</code> reads the server's own working directory, and when that cannot be read it is refused as <code>internal_error</code> naming the cause instead of searching an empty path.",
          "RPC context requests are registered before their tracked worker starts, so the inbox remains responsive and an immediate cancellation cannot race registration. <code>context/cancel</code> takes <code>{requestId}</code> on the same connection and acknowledges <code>cancellationRequested: true</code>, not completed termination. An unknown or finished id is refused as <code>request_not_running</code>; a duplicate active context id is refused as <code>request_already_running</code>. Shutdown announces cancellation to every owned context request before joining workers. Final source results remain the completion evidence; no deadline is implied.",
          "A terminal context response is emitted only after its active request lease is released. Once that response arrives, the same connection can reuse its id; a completed request cannot be cancelled and reports <code>request_not_running</code> instead of acknowledging a cancellation that did nothing.",
          "Agent-tool, interactive and automatic turn-prologue requests carry their caller's cancellation flag. Stopping a turn therefore also cancels its context preparation rather than leaving that preparation with an unrelated token. A source cancelled before execution answers unavailable with that cause; file traversal and ranking also check cancellation between entries and sections. A kernel file read already in progress still has to return.",
          "The fleet client does not hold its shared cache mutex while connecting or migrating. Concurrent memory callers wait for the initializer through an event-driven condition variable; a cancelled waiter leaves that wait, and failed or panicking initialization wakes the others. Connection establishment, migrations and memory queries carry the caller's cancellation to Stado; cancellation of one request does not poison the shared client for later work.",
          "Transcript searches and probes own their child process group until completion or cancellation, including child reaping and pipe drainage. Ground-truth search and health requests observe the same cancellation through both the HTTP request and response-body read. HTTP refusals retain status and body, malformed answers retain the decoding cause and received text, and transport failures retain their nested causes in the named unavailable source. These terminal source results, not the cancellation acknowledgement, establish completion.",
          "The files probe uses the same caller cancellation as a recommendation. A scan stopped between entries is reported as unavailable with the cancellation cause, not as a successful partial corpus or as an empty directory.",
          "Source inspection emits a separate wait for each probe, naming its source and workspace before reading it. Each wait ends with the probe's actual availability or failure detail; a cancelled probe reports cancellation even when no dependency request was started. This gives the same source-level visibility to <code>jeden context sources</code> and the native Context screen as to recommendations.",
        ],
      },
      {
        title: "Real dependency journey",
        paragraphs: [
          "<code>tests/context/dependencies.mjs</code> runs two concurrent <code>context recommend --source all</code> calls against the real fleet memory database and Transcript Lake, from a new workspace containing one searchable file. It checks that both calls return that file, that the new repository scope exposes no memory from another workspace, and that an unknown source is refused without returning narrowed advice. An unavailable memory database or archive fails the journey; neither is simulated.",
          "Declare <code>JEDEN_BIN</code>, <code>JEDEN_TEST_SOURCE_REVISION</code> and <code>JEDEN_TEST_CONTEXT_LIMIT</code>, then run <code>node tests/context/dependencies.mjs</code> from the checkout. The candidate must be committed and its binary must embed that clean source revision. Reports, command exit statuses, binary and test hashes, and both output streams are retained under <code>.build/context-dependencies</code>. The optional ground-truth source reports its own state; the journey does not qualify an unconfigured index or a graphical client's rendering.",
          "<code>tests/context/cancellation.mjs</code> proves that a source which never answers no longer holds its caller. It starts the real <code>jeden rpc</code>, points the workspace's <code>context.advisor.groundTruthUrl</code> at a listener on this machine's loopback interface that accepts and never answers, sends <code>context/recommend</code> with <code>sources: \"ground-truth\"</code>, and once the source's GET has connected sends <code>context/cancel</code> with that request's id. The answer must list ground-truth as unavailable and unanswered with <code>cancelled by its caller</code> and the URL as its cause, standard error must carry the <code>czekam: GET &lt;url&gt;</code> and <code>context advisor source ground-truth</code> waits, a second cancel of the finished id must be refused, and the same connection must then answer <code>context/sources</code>. Declare <code>JEDEN_BIN</code> and <code>JEDEN_TEST_SOURCE_REVISION</code> and run <code>node tests/context/cancellation.mjs</code>; the report, every RPC frame and the server's standard error are retained under <code>.build/context-cancellation</code>.",
        ],
      },
      {
        title: "Configuration",
        paragraphs: [
          "Every key below is in <code>jeden config list</code> and can be set per user in <code>~/.jeden/config.yml</code> or per project in <code>&lt;cwd&gt;/.jeden/config.json</code>.",
          "<code>context.advisor.enabled</code> (default <code>true</code>) appends the block to every task prompt once <code>context.advisor.limit</code> is declared: that limit is how many recommendations one answer carries, and while it is unset a task prompt gets no recommendations (standard error says so) and a recommendation that states no <code>--limit</code> is refused naming the key. <code>context.advisor.maxChars</code>, when declared, is the injected block's character budget; unset, the block is whole. There is no deadline setting: every source runs to completion.",
          "<code>context.advisor.sources</code> (default <code>files,memory</code>) selects the sources every run consults. <code>context.advisor.roots</code> (default empty, meaning the project and <code>~/.jeden</code>) declares the roots the files source walks. <code>context.advisor.fileExtensions</code> (default empty, meaning every readable text file) narrows it to named extensions. <code>context.advisor.groundTruthUrl</code> and <code>context.advisor.transcriptLakeBin</code> point the two external sources somewhere other than their defaults.",
        ],
        commands: [
          {
            label: "Declare a wider corpus",
            code: 'jeden config set context.advisor.roots ".@6:~@1:~/agents@1"\njeden config set context.advisor.fileExtensions "md,rs,ts,swift"\njeden context sources',
          },
        ],
      },
    ],
  },
];
