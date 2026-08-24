# Quick start

How do you go from a clone to one verified agent answer? This page is the
one happy path: build from source, provide the model-router environment,
run the setup wizard once, execute one one-shot task, and read the recorded
session. Everything else — the machine interfaces, the ledger formats, the
credential rule — lives in [rpc](rpc.md), [sessions](sessions.md), and
[model-access](model-access.md).

## Build from source

Supported platforms are `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`,
and `x86_64-pc-windows-msvc`; any platform with a Rust toolchain can build
from source:

```sh
git clone https://github.com/wisent-ai/jeden.git && cd jeden
cargo build --locked --release   # or: bin/jeden-rust, which rebuilds stale source binaries
```

`bin/jeden-rust` builds the `jeden` and `jeden-sandbox-helper` binaries and
then executes `target/release/jeden`. For commands that need the model
runtime (interactive, `run`, `rpc`, `acp`, `headless`, `resume`,
`recall_conversation`, `token`, `probierz`), it first bootstraps missing
credentials through `scripts/run-with-stado.sh` when a Skarbiec token file
is present — see [model-access](model-access.md) for exactly which item each
credential is read from.

## Provide the model environment

Required environment for real model calls:

```sh
WISENT_APP_AGENT_AUTH_SECRET=<signing-credential>
BRAMA_URL=<brama-model-router-url>
# Set only when Brama requires its distinct bearer.
BRAMA_TOKEN=<brama-bearer>
WISENT_APP_AGENT_ID=wisent-app
```

`WISENT_APP_AGENT_AUTH_SECRET` is read from the process environment only and
is never written to disk. On a configured Wisent workstation
`bin/jeden-rust` obtains it automatically; anywhere else, export it
yourself. At startup Jeden also loads `<cwd>/.env`, `.env.local`,
`.env.production`, `.env.vercel`, then `~/.jeden/.env`, and each file sets
only variables that are still unset — the process environment always wins.
The full variable list is in [configuration](configuration.md).

## Run setup once

```sh
jeden            # opens the welcome view; run /setup to connect the model router
```

`/setup` is an idempotent wizard covering the Brama URL, agent id, default
model, and preferences. It writes only non-secret keys to `~/.jeden/.env` at
mode `0600` and never writes the secret. `/setup validate` probes live state
and ends with a smoke call. The first-use journey (`/onboarding`) is
separate and always runs from the definition compiled into the binary, so it
works offline.

## Execute one task

A successful setup is observable:

```sh
jeden run "Respond exactly: OK"   # expected output: OK
```

`jeden run` is one-shot: the final answer prints on stdout and the session
is recorded. Write and command tools stop for approval unless
`--allow-write` or `--allow-command` is passed:

```sh
jeden run "create notes.txt" --allow-write
jeden run "inspect the build" --allow-command
```

## Read what happened

```sh
jeden sessions
jeden show <session>
```

Every run leaves a session directory under `~/.jeden/sessions/` with
`state.json` and an append-only `transcript.jsonl`; token and cost
accounting for the project accumulates in `<cwd>/.jeden/usage.json`. Both
formats are in [sessions](sessions.md).

## When it does not work

- `BRAMA_URL is required` — the Brama endpoint is not configured; run
  `/setup` or export the variable, then rerun.
- `WISENT_APP_AGENT_AUTH_SECRET` missing — launch through `bin/jeden-rust`
  or `scripts/run-with-stado.sh`; both obtain the signing credential without
  writing it to disk.
- Anything else — `jeden doctor` returns a per-service JSON health report
  and exits non-zero when any probe is unavailable; `/setup validate` runs
  an end-to-end probe. Every refusal sentence and its repair is in the
  [runbook](runbook.md).

That is the whole path. The command vocabulary is in `jeden --help`; the
released vocabulary is frozen in `released-surface.json` at the repository
root. To drive the same run loop from an editor or a program, continue with
[rpc](rpc.md).
