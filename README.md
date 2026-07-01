# Jeden

Jeden is Wisent's private agent harness. It keeps Wisent's model routing, policy, and domain tools under our control instead of inheriting a generic coding-agent policy stack.

## Design contract

Jeden separates four planes:

1. **Inference** — every model call goes through the Wisent model router using HMAC-signed OpenAI-compatible chat completions.
2. **Policy** — the harness prompt is short, local, and explicit: no unrequested tests, no docs unless requested, no silent substitution, no shell by default.
3. **Tools** — tools are a small allowlisted registry with path-jail enforcement. Initial tools are filesystem-only: list, read, search, and opt-in write.
4. **Run loop** — the model must emit strict JSON actions. Invalid JSON is a hard failure.

## Current scope

The initial private version is intentionally small:

- `jeden run "task"` starts a bounded JSON tool loop.
- Model calls use `MODEL_ROUTER_URL`, `WISENT_APP_AGENT_ID`, and `WISENT_APP_AGENT_AUTH_SECRET`.
- The default model is `claude-code-subscription`.
- Writes require `--allow-write`; shell execution is not present.
- No tests are included; repository hooks and `npm run check` are the quality gate.

## CLI

```sh
jeden run "summarize src/lib/api/model-router-hmac.ts" --cwd ../content-platform
jeden run "create notes.txt with hello" --cwd /tmp/sandbox --allow-write
```

Required env for real model calls:

```sh
WISENT_APP_AGENT_AUTH_SECRET=...
MODEL_ROUTER_URL=https://model-router-1080673333190.us-central1.run.app
WISENT_APP_AGENT_ID=wisent-app
```

## JSON action protocol

The model must answer with one JSON object:

```json
{"action":"tool","tool":"read_file","input":{"path":"package.json"}}
```

or:

```json
{"action":"final","text":"Done."}
```

Tool results are fed back into the next model turn. The loop stops on `final` or when `--max-steps` is reached.
