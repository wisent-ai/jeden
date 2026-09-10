import { sessionCommands } from "./commands/sessions.mjs";
import { operationCommands } from "./commands/operations.mjs";
import { managementCommands } from "./commands/management.mjs";
import { completionCommands } from "./commands/completion.mjs";
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
  ...sessionCommands,
  ...operationCommands,
  ...managementCommands,
  ...completionCommands,
];

const groups = [
  ["Run and automation", ["run", "pursue", "rpc", "headless", "acp", "collab-relay"]],
  ["Sessions and artifacts", ["sessions", "show", "export", "artifacts", "artifact", "search-sessions", "resume", "recall_conversation"]],
  ["Runtime and operations", ["tools", "update", "doctor", "conformance", "probierz", "capabilities", "token", "stats", "gallery"]],
  ["Configuration", ["workspace", "workspace/status", "workspace/discover", "workspace/adopt", "config", "config/list", "config/path", "config/get", "config/set", "config/reset", "contracts"]],
  ["Shell completions", ["completions", "completions/bash", "completions/zsh", "completions/fish"]],
  ["Retained tasks", completionCommands.map((command) => command.path)],
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
