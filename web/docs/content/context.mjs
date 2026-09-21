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
        "Ranked context recommendations with exact locators, injected into every turn and callable from the CLI, the agent tool set, Jeden Desktop and an Omp session.",
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
          "<strong>files</strong> reads everything readable under the declared roots — documentation, source code, configuration, manifests — and ranks it in chunks. Markdown is cut at its headings, because that is where its meaning starts; every other file is cut into forty-line windows titled by the window's first line at column zero, which in source is the declaration the rest of the window belongs to. The locator is <code>path:first-last</code>, so the answer can be read directly. Roots are declared in <code>context.advisor.roots</code> as colon-separated <code>path</code> or <code>path@depth</code> entries and default to the project plus <code>~/.jeden</code>. A file with a zero byte in its head is not text and is skipped, a file over 256 KB is skipped, and <code>.gitignore</code> is respected.",
          "<strong>memory</strong> recalls what earlier sessions wrote down for this working directory. The store and its ranking belong to Jeden's memory subsystem — <code>~/.jeden/memory.sqlite3</code>, scope <code>repo:&lt;cwd&gt;</code> — and the locator is <code>memory:&lt;id&gt;</code>. An empty store is reported as an empty store, not as a missing match.",
          "<strong>transcripts</strong> asks Transcript Lake, which owns the masked canonical archive of every recorded agent session. The advisor runs that product's own <code>search</code> command rather than reading its files, so the masking stays applied; the locator is <code>session:&lt;id&gt;</code>, which <code>jeden show</code> and the <code>recall_conversation</code> tool read.",
          "<strong>ground-truth</strong> asks the Wisent ground-truth index, which owns cited answers across the organization's repositories. The locator is the citation it returns — <code>repo/path@commit:first-last</code>. The endpoint comes from <code>context.advisor.groundTruthUrl</code>, or from <code>WISENT_GROUND_TRUTH_API</code> or <code>GROUND_TRUTH_API</code> when that is empty; <code>jeden context sources</code> reports which of the three it used.",
        ],
      },
      {
        title: "Every source answers, and each one runs to completion",
        paragraphs: [
          "<code>context.advisor.sources</code> defaults to <code>files,memory</code>, the two that answer from local state, and the sources run concurrently, so a turn waits for the slowest rather than for their sum. <code>--source all</code> adds the archive and the ground-truth index to one call.",
          "Nothing cuts a source short. There is no deadline setting and no <code>--timeout-ms</code> flag, because a guessed interval reports nothing and explains nothing: a search that takes ten seconds takes ten seconds and answers. On a large Transcript Lake archive that search is measured in seconds, and that is what a turn with <code>transcripts</code> selected pays. Choosing which sources a run consults is therefore the decision that replaces the guess a deadline used to make: measured here, one archive search took 30 s against 1 s for the two local sources.",
          "An unknown source name is refused rather than dropped: <code>unknown source(s): nonsense. Known sources: files, ground-truth, memory, transcripts</code>. A narrowed answer is never the result of a typo.",
        ],
        commands: [
          {
            label: "Ask every source, or only the fast ones",
            code: 'jeden context recommend "why did the release agent quarantine that host" --limit 8\njeden context recommend "why did the release agent quarantine that host" --source files,memory',
          },
        ],
      },
      {
        title: "Reading a short list",
        paragraphs: [
          "Every answer carries a state for every source it consulted: <code>available</code> or <code>unavailable</code>, the observed reason, how much was considered, how much was returned, and how long it took. A list that is short because a source refused therefore looks different from a list that is short because nothing matched.",
          "The files source also says when its own walk was cut: a root larger than the caps allow produces a partial corpus, and the detail then reads <em>the walk stopped at its cap, so this is a partial corpus</em>.",
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
        title: "The same advisor in an Omp session",
        paragraphs: [
          "Omp is not Jeden's to patch, and it does not need to be: it loads custom tools from <code>~/.omp/agent/tools/*.ts</code>. <code>jeden context install --omp</code> renders a <code>context_recommend</code> tool into that directory, bound to the absolute path of the Jeden binary that rendered it, and that tool calls <code>jeden context recommend --json</code>. One implementation, two harnesses.",
          "<code>jeden context installed --omp</code> reports <code>current</code>, <code>stale</code> or <code>absent</code> and exits non-zero unless the installed file is exactly what this binary renders, so an upgrade that changes the tool is visible instead of silent. <code>--file &lt;path&gt;</code> writes or checks any other location, which is how the product's own tests drive it.",
        ],
        commands: [
          {
            label: "Install and verify",
            code: "jeden context install --omp\njeden context installed --omp",
          },
        ],
      },
      {
        title: "Every surface",
        paragraphs: [
          "<strong>CLI:</strong> <code>jeden context recommend</code>, <code>prompt</code>, <code>sources</code>, <code>install</code>, <code>installed</code>. A bare first word is the task, so <code>jeden context \"why does signing fail\"</code> works.",
          "<strong>Interactive:</strong> <code>/context &lt;task&gt;</code> renders the same recommendations; bare <code>/context</code> keeps reporting the live window size.",
          "<strong>Agent tool:</strong> <code>context_recommend</code> takes <code>query</code>, and optionally <code>limit</code>, <code>sources</code>. It is a read-tier tool, because it does what the prologue already does unapproved.",
          "<strong>RPC and Jeden Desktop:</strong> <code>context/recommend</code> and <code>context/sources</code> answer the same objects the CLI prints with <code>--json</code>. Desktop's Context screen is that RPC, not a parsed command line.",
        ],
      },
      {
        title: "Configuration",
        paragraphs: [
          "Every key below is in <code>jeden config list</code> and can be set per user in <code>~/.jeden/config.yml</code> or per project in <code>&lt;cwd&gt;/.jeden/config.json</code>.",
          "<code>context.advisor.enabled</code> (default <code>true</code>) appends the block to every task prompt. <code>context.advisor.limit</code> (default <code>6</code>) is how many recommendations one answer carries. <code>context.advisor.maxChars</code> (default <code>6000</code>) is the injected block's budget. There is no deadline setting: every source runs to completion.",
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
