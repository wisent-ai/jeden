# Mode state

Where does a project remember that plan mode is on, what the pinned goal is,
and which tools are pre-approved? In `<cwd>/.jeden/mode-state.json` — the
project's durable mode document, defined by `ModeState` in
`rust/slash/state.rs` and shared by every interface that runs in that
checkout.

## Fields

| Key | Shape | Meaning |
|---|---|---|
| `plan` | `{enabled, latestPlan}` | plan mode and the last recorded plan text |
| `goal` | `{enabled, paused, objective, budget, auto}` | the pinned durable objective; `auto` lets Oko's goal-lifecycle model start and finish goals from classified prompts |
| `guidedGoal` | `{active, roughObjective}` | guided-goal capture state |
| `loop_mode` | `{enabled, remaining, until, prompt}` | continuation loop |
| `fast` | `{enabled, serviceTier}` | fast mode; `serviceTier` defaults to `"priority"` and feeds the model request's service tier when enabled |
| `advisor` | `{enabled, model, lastReview}` | advisor review state |
| `force` | `{tool, prompt}` or `null` | forced next tool; armed by `/force <tool>` and cleared when the next turn injects it |
| `lastFailedTask`, `lastTask` | strings | last submitted task texts (feed `/retry`) |
| `compact` | bool | compact rendering |
| `shake` | string | UI shake state |
| `todos[]` | `{text, status, createdAt}` | project todos |
| `branches[]` | `{id, title, createdAt, path, roadmapItem}` | session branches |
| `activeRoadmapItem` | string or null | pins session artifacts and branches to a roadmap item |
| `lastSessionPath` | path or null | most recent session directory; `jeden run /compact` and friends open it |
| `tools` | `{approvalMode, approval{}}` | approval-policy overrides per tool |

Every field is `#[serde(default)]`: a missing or unparsable file reads as
the default state — readers never fail on absence.

## Write protocol

Writers serialize through a lock file and commit atomically
(`rust/slash/state.rs`):

1. Acquire `.jeden/.mode-state.lock` — create-new containing the writer's
   pid, retried up to 500 times at 10 ms; a bounded wait that expires
   refuses with `timed out waiting for mode-state lock <path>`.
2. Write `.mode-state.json.tmp-<pid>-<nonce>`, flush, `sync_all`.
3. Rename over `mode-state.json` and sync the directory. A crash at any
   point leaves the previous document intact; the temp file is removed on
   error. A path with no parent directory refuses with
   `mode-state path has no parent`.

## Who reads and writes it

- Slash commands: `/plan`, `/goal`, `/guided-goal`, `/loop`, `/fast`,
  `/todo`, `/force`, `/approval`, `/retry`, `/branch`.
- The model request: when `fast.enabled` is true, `fast.serviceTier` is the
  service-tier fallback after `JEDEN_SERVICE_TIER` and `MODEL_SERVICE_TIER`
  (`rust/agent/runtime/routing.rs`) — environment wins over mode state.
- One-shot session commands: `jeden run "/compact"`, `/handoff`,
  `/checkpoint`, `/rewind`, `/context` open `lastSessionPath`; with no prior
  session they refuse with
  `No prior session found. Run a task first, then use a session command.`
- New session artifacts are stamped with `activeRoadmapItem`.

## Neighbors in `.jeden/`

Two more per-project files sit beside it: `config.json`
([configuration](../configuration.md)) and
`subscription-cooldowns.json` — the durable `Retry-After`-bounded cooldown
store for subscription routing (`rust/routing/store.rs`), shape
`{"version": 1, "entries": [{"target": ..., "untilMs": ...}]}`, written with
the same tmp-fsync-rename discipline, whose refusals appear in the
[runbook](../runbook.md).

## Not to be confused with

- **[Session](session.md) state (`state.json`)** — per-session metadata under
  the session root; mode state is per-project and survives across sessions.
- **[Configuration](../configuration.md)** — operator-declared settings;
  mode state is machine-written interaction state.
