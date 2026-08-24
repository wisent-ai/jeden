# What is Jeden

What is Jeden, and what is the mental model for reading everything else in
these docs? Jeden is the local coding-agent harness a developer actually
runs: a single local process that takes a task, drives a model through
jailed, approval-gated tools against a project checkout, and records
everything it did in durable local ledgers. The whole product is three
moving parts — one run loop, a jailed tool registry, and ledgers on the
operator's disk.

## One run loop

A task enters through one interface — the interactive terminal, `jeden run`,
`jeden pursue`, `jeden rpc`, `jeden acp`, `jeden headless`, or an SDK — and
one run loop drives it to completion. The loop sends the conversation and
the derived tool schemas to Brama, receives either a final answer or tool
calls, executes each tool locally, appends the outcome to the session
ledger, and repeats until the model answers. The model may return native
tool calls or strict JSON actions; both enter the same local execution loop
(the wire contract is [JSON_ACTION_PROTOCOL](JSON_ACTION_PROTOCOL.md)).
Nothing but that local process reads the checkout.

Inference is reachable only through Brama: Jeden carries no provider API key
and no provider SDK, and every request is HMAC-signed with a credential that
exists only in the process environment. See
[model-access](model-access.md).

## Jailed, approval-gated tools

Tools come from a small allowlisted registry that enforces path jails and
write or command permission. Every file write or shell command pauses for
interactive approval unless explicitly enabled with `--allow-write`,
`--allow-command`, or `--yolo`; destructive confirmations default to
**Cancel**. Project hooks in `.jeden/hooks.json` run only with
`--allow-command`, so a cloned repository cannot silently execute shell.
Tool-initiated network access is checked against the execution grant's host
and port allowlist. File mutations are guarded by the digest or snapshot tag
returned by `read_file`, and everything fails closed: without `BRAMA_URL`
the run stops with `BRAMA_URL is required` and no model call is made.

## Durable local ledgers

Everything durable is a file the operator owns; Jeden uploads none of it.

- `~/.jeden/sessions/<id>/` — one directory per session: `state.json` plus
  `transcript.jsonl`, an append-only ledger of sequenced, parent-linked,
  checksum-sealed events, validated on read and `fsync`ed on every append.
- `<cwd>/.jeden/usage.json` — one event per model completion: tokens, the
  Brama-catalog-priced cost breakdown, and the served billing target.
- `<cwd>/.jeden/mode-state.json` — the project's durable mode state: plan,
  goal, loop, fast tier, todos, branches, approval overrides.

All three are documented in [sessions](sessions.md).

## What Jeden is not

Jeden is not a hosted or multi-tenant service; it is a local harness, usable
without a hosted Wisent account. It does not provide model inference itself;
Brama (or another compatible OpenAI-style endpoint) serves the requests, and
credential rotation and revocation are owned by Skarbiec — the harness holds
no credential store and writes no secret to disk. It owns nothing about the
fleet: Stado builds and promotes its release archives, but placement,
queues, and fleet state are Stado's. It never handles cardholder data;
Wisent Platform Billing owns billing. And it does not define autonomous
objective pursuit: Pursuit owns intent distillation, outcome contracts, and
repair loops, while `jeden pursue` supplies Brama-backed conversations and
approval-gated tools to the accepted execution stages.

## The first three commands

```sh
jeden
```

Opens the interactive terminal in the current checkout; `/setup` connects
the model router. The end-to-end path is [quick-start](quick-start.md).

```sh
jeden run "summarize package.json"
```

One-shot mode: the final answer prints on stdout and the session is
recorded. Grants are explicit per invocation.

```sh
jeden sessions
```

Lists recorded sessions. `jeden show`, `export`, `artifacts`, and `resume`
inspect or continue them; see [sessions](sessions.md). Machine access to the
same run loop is [rpc](rpc.md); the knobs are
[configuration](configuration.md).

## Reading map

- Concepts, one page per noun: [session](concepts/session.md),
  [event envelope](concepts/event-envelope.md),
  [ledger](concepts/ledger.md), [outbox](concepts/outbox.md),
  [mode state](concepts/mode-state.md),
  [usage record](concepts/usage-record.md),
  [capability](concepts/capability.md),
  [headless tenant](concepts/headless-tenant.md).
- Interfaces: the full [CLI](cli.md), the [RPC protocol](rpc.md), the
  mutual-TLS [headless service](headless.md).
- Executed walkthroughs:
  [offline refusals](walkthrough-offline-refusals.md) and
  [session export and recovery](walkthrough-session-export.md), with
  runnable scripts in `examples/`.
- When something refuses: the [runbook](runbook.md).
