const selection = "Optional <code>--session &lt;id-or-path&gt;</code> selects a recorded session; otherwise the workspace's latest session is used. <code>--json</code> returns the authoritative completion snapshot.";
const controls = "An original request ID controls every unfinished task it owns, including an unplanned request when model intake is unavailable. A task ID controls that task only. <code>--revision</code> must match the latest snapshot and <code>--reason</code> must explain the operator's action.";
const failures = [
  "Without a selected or current session: <code>no active session; use --session &lt;id-or-path&gt;</code>.",
  "Missing control fields: <code>task control requires --revision</code> or <code>task control requires --reason</code>; an empty explanation is refused as <code>task control requires a nonempty operator reason</code>.",
  "A stale revision is refused as <code>completion state changed: expected revision &lt;expected&gt;, found &lt;actual&gt;</code>; refresh the snapshot before choosing another action.",
  "Unknown IDs are refused as <code>unknown task or request: &lt;id&gt;</code>. Completed or cancelled work is terminal and cannot be resumed by changing its status.",
];

export const completionCommands = [
  {
    path: "todo",
    invocation: 'jeden todo [list|add "request"|pause <id>|resume <id>|cancel <id>|continue] [--session <id-or-path>] [--revision <n> --reason "text"] [--json]',
    purpose: "Inspect and control every retained user request and its verified task state.",
    inputs: [selection, controls],
    effect: "Stores requests, acceptance criteria, task states, independent observations and actual failures in the session's atomic <code>completion.json</code>. The agent's todo done action requests verification; it cannot mark work complete. Starting another question, clearing model context, or resuming a recorded session does not discard retained obligations. Legacy task claims are imported for verification rather than trusted as successful.",
    refusals: [...failures, "There is no operator <code>done</code> command: <code>Task completion requires independent verification; there is no operator done command.</code>"],
  },
  {
    path: "todo/list",
    invocation: "jeden todo list [--session <id-or-path>] [--json]",
    purpose: "Read retained requests, tasks, acceptance requirements and completion evidence.",
    inputs: [selection],
    effect: "Prints status, revision, original requests, acceptance criteria, failed operations and verified session/event references. <code>complete: false</code> remains explicit when there is unplanned, pending, paused, interrupted, blocked or unverified work. Reading a legacy workspace list migrates that superseded list before displaying its session-owned replacement.",
    refusals: [failures[0], "Invalid or unreadable completion files return their storage or schema error, never an empty successful task list."],
  },
  {
    path: "todo/add",
    invocation: 'jeden todo add "request" [--session <id-or-path>] [--json]',
    purpose: "Record a user request without executing it or inventing its acceptance result.",
    inputs: [selection, "Required: nonempty request text. When no active session exists, a new durable session is created."],
    effect: "Appends an unplanned request and preserves every earlier request. It does not call a model. Continue later to record the acceptance criteria independently and execute the request.",
    refusals: ["Missing text returns the todo usage. Storage failures are reported without claiming the request was completed."],
  },
  ...[
    ["pause", "Pause retained work without losing it.", "Marks the selected unfinished work paused. It remains incomplete and the execution agent cannot resume it itself."],
    ["resume", "Make paused retained work eligible to run again.", "Returns the selected work to pending; it does not execute it. Use todo continue or Continue retained work in the graphical session to run it with the current grants."],
    ["cancel", "Withdraw exactly the work the operator selected.", "Records the operator's reason and cancellation without deleting the original request, completed evidence or other work. A cancelled request is not reported as verified implementation."],
  ].map(([action, purpose, effect]) => ({
    path: `todo/${action}`,
    invocation: `jeden todo ${action} <task-or-request-id> --revision <n> --reason "text" [--session <id-or-path>] [--json]`,
    purpose,
    inputs: [selection, controls],
    effect,
    refusals: failures,
  })),
  {
    path: "todo/continue",
    invocation: "jeden todo continue [--session <id-or-path>] [--allow-write] [--allow-command] [--model name] [--max-steps n] [--json]",
    purpose: "Inspect previous results and continue retained work without adding a synthetic user request.",
    inputs: [selection, "The recorded session workspace is used. Write and command permissions are explicit for this invocation; continuation never expands them."],
    effect: "Opens the retained session, records any missing acceptance criteria and independently inspects the existing results before execution. A proposed final answer is withheld while any actionable request remains unfinished. Transport failure, operator interruption or an execution limit leaves an incomplete snapshot with its actual cause. Complete work returns without another model call.",
    refusals: ["Paused work is refused as <code>Retained work is paused; resume its tasks before continuing.</code>.", "Unavailable Brama access or independent review is a failed run with retained work, not completion. <code>execution_limit</code> means paused and <code>turn_cancelled</code> means interrupted; neither status means done."],
  },
];
