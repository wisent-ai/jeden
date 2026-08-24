# Walkthrough: an offline session, refusals and all

Can you trust what Jeden does when it has no model credential at all? This
walkthrough runs the harness fully offline — no `BRAMA_URL`, no signing
secret, an isolated `$HOME` — and shows that every refusal is exact,
fail-closed, and still ledgered. Every command below was executed against
`jeden 0.1.1+dev.356.9b82942` built from source; output is pasted
unedited (timestamps and suffixes will differ on your machine).

## Setup: an isolated, offline environment

```sh
export HOME=$(mktemp -d)          # isolate ~/.jeden entirely
mkdir -p $HOME/project && cd $HOME/project
unset BRAMA_URL BRAMA_TOKEN WISENT_APP_AGENT_AUTH_SECRET \
      STADO_MODEL_ROUTER_URL STADO_MODEL_ROUTER_TOKEN JEDEN_MODEL
jeden --version
```

```
jeden 0.1.1+dev.356.9b82942
```

## 1. The one-shot refusal

```sh
jeden run "Respond exactly: OK"; echo "exit=$?"
```

```
Error: BRAMA_URL is required; configure the Brama model-router service URL
exit=1
```

Fail-closed: no model call was attempted, no network was touched. But the
attempt was still recorded:

```sh
jeden sessions
```

```
1787608963-JqlaM8
```

```sh
cat $HOME/.jeden/sessions/1787608963-JqlaM8/transcript.jsonl | python3 -c \
  "import sys,json; [print(json.loads(l)['payload']['type']) for l in sys.stdin]"
```

```
user
run_error
```

The second event's payload carries the refusal verbatim:

```json
{"type":"run_error","data":{"message":"BRAMA_URL is required; configure the Brama model-router service URL"}}
```

A failed run is history too — sequenced, parent-linked, checksummed
([concepts/ledger](concepts/ledger.md)).

## 2. The same refusal over RPC

Drive the machine interface with a here-stream ([rpc](rpc.md)):

```sh
{ printf '%s\n' '{"id":1,"method":"initialize"}'
  printf '%s\n' '{"id":2,"method":"session/new","params":{"cwd":"'$HOME'/project"}}'
  printf '%s\n' '{"id":3,"method":"session/prompt","params":{"sessionId":"session-1","prompt":"Respond exactly: OK"}}'
  sleep 2
  printf '%s\n' '{"id":4,"method":"session/status","params":{"sessionId":"session-1"}}'
  printf '%s\n' '{"id":5,"method":"shutdown"}'
} | jeden rpc
```

```
{"type":"ready","protocol":"jeden-rpc","version":1,"capabilities":{"protocolVersion":1,"prompt":true,"abort":true,"resume":true,"eventSubscription":true,"elicitation":true,"approval":true,"transports":["ndjson","acp"]}}
{"id":1,"result":{"protocol":"jeden-rpc","capabilities":{"protocolVersion":1,"prompt":true,"abort":true,"resume":true,"eventSubscription":true,"elicitation":true,"approval":true,"transports":["ndjson","acp"]}}}
{"id":2,"result":{"sessionId":"session-1","sessionPath":"/tmp/.../.jeden/sessions/1787608989-tig1z6"}}
{"method":"session/event","params":{"requestId":"3","kind":"status","message":"thinking (step 1/unbounded)"}}
{"method":"session/event","params":{"requestId":"3","kind":"error","message":"BRAMA_URL is required; configure the Brama model-router service URL"}}
{"id":3,"error":{"code":"prompt_failed","message":"BRAMA_URL is required; configure the Brama model-router service URL"}}
{"id":4,"result":{"activeRequestIds":[]}}
{"id":5,"result":{"shuttingDown":true}}
```

Note the order: the durable session directory exists before the prompt, the
`status` event streams, the terminal `error` event closes the stream, then
the wire response repeats the same sentence as `prompt_failed`. The rest of
the RPC error surface behaves the same offline as on. Feeding these four
lines —

```
{"id":10,"method":"bogus"}
{bad
{"id":12,"method":"session/new","params":{"cwd":123}}
{"id":13,"method":"session/status","params":{"sessionId":"session-9"}}
```

— answers:

```
{"id":10,"error":{"code":"method_not_found","message":"unknown method: bogus"}}
{"id":null,"error":{"code":"malformed_json","message":"key must be a string at line 1 column 2"}}
{"id":12,"error":{"code":"invalid_params","message":"invalid type: integer `123`, expected path string"}}
{"id":13,"error":{"code":"unknown_session","message":"unknown session: session-9"}}
```

(`session/prompt` against an unknown id reports the same sentence under
`prompt_failed`, because the prompt as a whole failed.)

## 3. Every credential-bearing command refuses the same way

```sh
jeden token
```

```
Error: BRAMA_URL is required; configure the Brama model-router service URL
```

```sh
jeden pursue "make a demo"
```

```
Error: BRAMA_URL is required; configure the Brama model-router service URL; receipt: /private/tmp/.../project/.pursuit/runs/1787609193248-6163-0/receipt.json
```

Even the refused pursuit writes its receipt — evidence before convenience.

## 4. `doctor` names what is missing

```sh
jeden doctor; echo "exit=$?"
```

Abridged to the two failing probes (ten others were `healthy` or
`degraded`):

```json
{"schemaVersion":1,"healthy":false,"cwd":"/private/tmp/.../project","probes":[
 {"subsystem":"brama","state":"unavailable","active":false,"latencyMs":0,
  "detail":"BRAMA_URL is required; configure the Brama model-router service URL", ...},
 {"subsystem":"weles","state":"unavailable","active":false,"latencyMs":0,
  "detail":"WISENT_PLATFORM_BILLING_URL is not configured", ...},
 {"subsystem":"storage","state":"healthy","detail":"durable write/read/remove succeeded", ...},
 ...]}
exit=1
```

`doctor` exits non-zero because a probe is `unavailable` — `degraded`
subsystems (here: `extensions` and `lsp` with
`no configured capability was discovered`) do not fail the gate.

## 5. What accounting looks like with zero spend

```sh
jeden stats --summary
```

```
0 events · 0 tokens · cost 0 · sessions 2
```

Sessions were recorded; not one token was spent, because not one request
left the machine. That is the offline contract: **no credential, no call,
full evidence**.

## Where to go next

- Read what those sessions contain:
  [walkthrough-session-export](walkthrough-session-export.md).
- Provide the real environment and run the happy path:
  [quick-start](quick-start.md).
- The refusal sentences and their repairs: [runbook](runbook.md).
- A runnable version of this audit:
  [examples/offline-refusal-audit.sh](examples/offline-refusal-audit.sh).
