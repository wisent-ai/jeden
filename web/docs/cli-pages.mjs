import { sessionCommands } from "./commands/sessions.mjs";
import { operationCommands } from "./commands/operations.mjs";
import { managementCommands } from "./commands/management.mjs";
import { completionCommands } from "./commands/completion.mjs";
import { contextCommands } from "./commands/context.mjs";
const ORIGIN = "https://jeden.wisent.com";
// The exit status every command shares, stated once rather than per page.
const EXIT_STATUS = "A command line that does not parse, names an unknown command, or that a command refuses as its own invocation (a missing, unexpected or invalid argument, a missing confirmation) prints the error and exits <code>2</code>; any other refusal or failure exits <code>1</code>.";

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
        bullets: [...refusals, EXIT_STATUS],
      },
    ],
  };
}

const commands = [
  ...sessionCommands,
  ...operationCommands,
  ...managementCommands,
  ...completionCommands,
  ...contextCommands,
];

// Each command declares the navigation group it belongs to; the tree is the
// groups in the order they first appear, so a page cannot be left out of the
// navigation and the navigation cannot name a page that does not exist.
const byPath = new Map(commands.map((entry) => [entry.path, entry]));
for (const entry of commands) {
  if (typeof entry.group !== "string" || entry.group.length === 0) {
    throw new Error(`CLI command page ${entry.path} declares no navigation group`);
  }
}
const groups = [...new Set(commands.map((entry) => entry.group))].map((title) => [
  title,
  commands.filter((entry) => entry.group === title).map((entry) => entry.path),
]);

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
        "Running <code>jeden</code> without a command opens the interactive terminal. <code>--cwd path</code>, <code>--model name</code>, <code>--max-tokens n</code>, <code>--max-steps n</code>, <code>--allow-write</code>, <code>--allow-command</code>, and <code>--yolo</code>/<code>--auto-approve</code> configure that root invocation. <code>--version</code>/<code>-V</code> prints the compiled version and <code>--help</code>/<code>-h</code> prints usage. <code>--help</code> or <code>-h</code> after any subcommand prints that subcommand's usage lines and never runs it.",
        "Unknown commands and unknown global options fail instead of falling through. Environment files load from the selected workspace before dispatched commands run.",
        "The prompt, approval questions and running turns share one terminal event stream. A key press, resize, worker output or worker completion triggers the next update; the busy indicator does not advance on a timer. Esc or Ctrl-C during a turn announces cancellation. Foreground commands and the external editor take ownership of stdin after the event reader is released.",
        "A terminal read failure reports <code>terminal event read failed: &lt;cause&gt;</code>; a closed stream reports <code>terminal event stream closed</code>. A rendering or input error cancels the running turn and releases queued approval replies before joining its worker. A worker panic reports <code>Turn thread panicked.</code> rather than leaving the prompt waiting for another key.",
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
