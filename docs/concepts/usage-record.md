# Usage record

What did the model work cost? Every completed model call appends one usage
record to `<cwd>/.jeden/usage.json` (`append_usage_event` in
`rust/agent/runtime/routing.rs`): tokens by class, the Brama-catalog-priced
cost breakdown, and — under subscription routing — the served billing
target.

## The document

```json
{"version": 1, "updatedAt": "<unix-seconds>", "events": [ ... ]}
```

A missing file is initialized as `{"version": 1, "events": []}`;
`updatedAt` is stamped on every append.

## One record

| Field | Type | Meaning |
|---|---|---|
| `at` | string | unix seconds at completion |
| `model` | string | the served model route |
| `serviceTier` | string or null | the request's service tier, null when empty |
| `inputTokens` | number | prompt tokens |
| `outputTokens` | number | completion tokens |
| `cacheReadTokens` | number | prompt tokens served from provider cache |
| `cacheWriteTokens` | number | prompt tokens written to provider cache |
| `totalTokens` | number | provider-reported total |
| `cost` | object, optional | `{input, output, cacheRead, cacheWrite, total}` — present when the model is priced in the Brama catalog; per-million-token rates applied to each token class |
| `billing` | object, optional | `{providerId, accountId, subscriptionId, quotaBucket, decisionId}` — present when subscription routing served the request; the audit trail back to the frozen route decision |

Failed attempts do not append usage records; the session ledger's
`model_retry` / `usage_error` [events](event-envelope.md) carry that story.

## Two scopes

- `<cwd>/.jeden/usage.json` — the project ledger, one file per checkout.
- `~/.jeden/usage.json` — the user-scope ledger.

`jeden stats` reports both (captured from a fresh project):

```json
"usage": {
  "project": {"path": ".../project/.jeden/usage.json", "events": 0, "tokens": 0.0, "cost": 0.0, "byModel": {}, "updatedAt": null},
  "user":    {"path": ".../.jeden/usage.json",         "events": 0, "tokens": 0.0, "cost": 0.0, "byModel": {}, "updatedAt": null}
}
```

## Readers

- `/usage show` — aggregates calls, tokens, and cost for the project.
- `/usage reset` — clears all recorded events (the one ledger in Jeden that
  is legitimately resettable; it is accounting, not history).
- `jeden stats` — `--json` (shape above, plus `quota` from Wisent Platform
  Billing when configured and `sessions.recent`), `--summary` (one line:
  `0 events · 0 tokens · cost 0 · sessions 2`), or `--serve [--port N]`, a
  local dashboard bound to `127.0.0.1`.
- The interactive status line sums the same project file.

## Not to be confused with

- **`model_usage` ledger events** — the same numbers recorded in the
  session's [ledger](ledger.md) for per-turn provenance; usage.json is the
  cross-session aggregate.
- **Quota** — `jeden stats` reports quota from Wisent Platform Billing when
  `WISENT_PLATFORM_BILLING_URL` is configured; otherwise
  `{"available": false, "reason": "WISENT_PLATFORM_BILLING_URL is not configured"}`.
- **The subscription cooldown store** — `.jeden/subscription-cooldowns.json`
  records when a quota-exhausted subscription may be retried; see
  [model-access](../model-access.md).
