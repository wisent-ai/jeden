# Walkthrough: reading, exporting, and breaking a session

A session directory is the product's memory. This walkthrough lists,
inspects, exports, searches, and resumes recorded sessions — then tears one
on purpose to show the recovery contract. Every command was executed against
`jeden 0.1.1+dev.356.9b82942`; output is pasted unedited. The sessions here
were produced by
[walkthrough-offline-refusals](walkthrough-offline-refusals.md).

## 1. List and show

```sh
jeden sessions
```

```
1787608963-JqlaM8
```

`jeden show <id>` prints the same JSON document `jeden export` produces. The
top of it, captured:

```json
{
  "id": "1787608989-tig1z6",
  "path": "/tmp/.../.jeden/sessions/1787608989-tig1z6",
  "state": {
    "version": 2,
    "id": "1787608989-tig1z6",
    "cwd": "/tmp/.../project",
    "startedAt": "1787608989",
    "activeLeaf": "event-1787608990-...",
    "lineage": null
  },
  "ledgerVersion": 2,
  "activeLeaf": "event-1787608990-...",
  "recoveredTruncatedTail": false,
  "events": [ ... ]
}
```

Each event in `events` is the full
[envelope](concepts/event-envelope.md) — id, parent, sequence, checksum,
[outbox](concepts/outbox.md) seeds — plus flattened compatibility fields
(`ts`, `type`, `data`) older consumers read. A missing id answers with JSON,
not a crash:

```sh
jeden show no-such-session
```

```
{
  "error": "session not found: /tmp/.../.jeden/sessions/no-such-session"
}
```

## 2. Export: JSON, markdown, HTML, file

```sh
jeden export 1787608989-tig1z6 --markdown | head -20
```

````
# Jeden session 1787608989-tig1z6

/tmp/.../.jeden/sessions/1787608989-tig1z6

## 1787608990 user

```json
{
  "task": "Respond exactly: OK",
  "cwd": "/tmp/.../project",
  "allowWrite": false,
  "allowCommand": false,
  "maxSteps": null,
  "maxTokens": null,
  "modelOnly": false,
  "goal": null
}
```
````

`--html` renders the same events as a self-contained page. A trailing
positional writes a file and prints its name:

```sh
jeden export 1787608989-tig1z6 out.json
```

```
out.json
```

In the interactive terminal, `/export [session] [--format json|text]
[--output file]` covers the slash side.

## 3. Artifacts and recall

```sh
jeden artifacts 1787608989-tig1z6      # name<TAB>bytes per artifact; empty here
jeden artifact 1787608989-tig1z6 <name> [output]
```

`artifact` canonicalizes and refuses traversal:
`artifact path escapes session: <name>`. `recall_conversation` renders the
transcript as markdown — the same view the agent's own `recall_conversation`
tool loads back into context.

## 4. Search across sessions

```sh
jeden search-sessions "BRAMA_URL"
```

```
1787609143-lUSkGm	1787609144	run_error	{"message":"BRAMA_URL is required; configure the Brama model-router service URL"}
1787608989-tig1z6	1787608990	run_error	{"message":"BRAMA_URL is required; configure the Brama model-router service URL"}
1787608963-JqlaM8	1787608964	run_error	{"message":"BRAMA_URL is required; configure the Brama model-router service URL"}
```

One line per matching session (`id`, `ts`, `type`, whitespace-collapsed
event text), newest first, first matching event per session.

## 5. Resume is a new session

```sh
jeden resume 1787608989-tig1z6 "continue"
```

Offline this refuses at the model boundary
(`Error: BRAMA_URL is required; ...`) — but the resume had already begun a
fresh child session, and its ledger shows how a resume starts:

```sh
cat $HOME/.jeden/sessions/1787609143-lUSkGm/transcript.jsonl | python3 -c \
  "import sys,json; [print(json.loads(l)['payload']['type']) for l in sys.stdin]"
```

```
context_snapshot
user
run_error
```

The parent's model-visible history is snapshotted first, then the new turn
begins. The parent session is untouched.

## 6. Tearing the tail — and the recovery contract

Simulate a crash mid-append by truncating the final line (no trailing
newline, malformed JSON tail):

```sh
python3 - <<'EOF'
p = ".../.jeden/sessions/1787608989-tig1z6/transcript.jsonl"
data = open(p, "rb").read()
open(p, "wb").write(data[:-30])
EOF
jeden show 1787608989-tig1z6 | python3 -c \
  "import json,sys; d=json.load(sys.stdin); print('recoveredTruncatedTail =', d['recoveredTruncatedTail'], '| events =', len(d['events']))"
```

```
recoveredTruncatedTail = True | events = 1
```

The reader kept every intact event and flagged the recovery. The contract
(`rust/session/store.rs`): such a ledger reads fine forever but refuses
further appends with

```
cannot append <dir>: transcript has a recovered truncated tail; resume into a child session
```

— resume it (step 5) and continue in the child. A torn line **in the
middle** is different and fatal on read:
`<path>:<line> is malformed JSON: <error>` — that is corruption, not a torn
tail; see the [runbook](runbook.md#a-session-refuses-to-load-or-to-append).

## What this proves

- Export is total: state, lineage, recovery flag, and every sealed envelope
  in one document.
- Search, recall, and artifacts read the same ledger the run loop wrote.
- Resume never mutates history — it forks it.
- A crash can only cost the torn tail, and the reader says so out loud.

A runnable version: [examples/session-export.sh](examples/session-export.sh).
