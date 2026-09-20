#!/usr/bin/env node
// The TypeScript SDK's envelope validators, driven for real against the
// canonical schema and the golden envelopes that ship beside it.
//
// The SDK used to repeat the protocol's field names as six hand-written
// arrays. `packages/sdk-typescript/scripts/protocol-keys.mjs` reads them out
// of `protocol/schema/v1/envelope.schema.json` instead, and this run is the
// gate: the generator in `--check` mode fails if the checked-in module has
// drifted from the schema, and the validators themselves are driven over
// every golden envelope plus the refusals that matter — a field the schema
// does not allow, and a required field left out.
//
// Usage: node tests/protocol/envelope-keys.mjs

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..");
const sdk = join(repoRoot, "packages", "sdk-typescript");

const failures = [];

function check(name, condition, detail = "") {
  if (condition) {
    console.log(`ok   ${name}`);
    return;
  }
  failures.push(`${name}${detail ? `: ${detail}` : ""}`);
  console.log(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

// 1. The checked-in key module still matches the schema it was read from.
let drift = null;
try {
  execFileSync(process.execPath, [join(sdk, "scripts", "protocol-keys.mjs"), "--check"], {
    cwd: sdk,
    stdio: "pipe",
  });
} catch (error) {
  drift = `${error.stderr ?? error}`.trim();
}
check("the generated key module matches the envelope schema", drift === null, drift ?? "");

// 2. The schema's own field names are the ones the SDK ships. The SDK is
// built first, so what is driven below is the module the product publishes,
// not the TypeScript sources.
execFileSync("npm", ["run", "build"], { cwd: sdk, stdio: "pipe" });
const schema = JSON.parse(
  readFileSync(join(repoRoot, "protocol", "schema", "v1", "envelope.schema.json"), "utf8"),
);
const keys = await import(join(sdk, "dist", "src", "protocol-keys.js"));
for (const [constant, def] of [
  ["REQUEST", "request"],
  ["REQUEST_META", "requestMeta"],
  ["RESPONSE", "response"],
  ["EVENT", "event"],
  ["ERROR_ENVELOPE", "error"],
  ["ERROR_BODY", "errorBody"],
  ["REPLAY_PARAMS", "replayParams"],
]) {
  const declared = Object.keys(schema.$defs[def].properties);
  const exported = keys[`${constant}_KEYS`];
  check(
    `${def} fields come from the schema`,
    JSON.stringify(declared) === JSON.stringify([...exported]),
    `${JSON.stringify(declared)} vs ${JSON.stringify(exported)}`,
  );
}

// 3. The validators accept every golden envelope.
const { isEnvelope, parseEnvelope } = await import(join(sdk, "dist", "src", "validators.js"));
const golden = JSON.parse(
  readFileSync(join(repoRoot, "protocol", "schema", "v1", "golden", "envelopes.json"), "utf8"),
);
check("the golden file carries envelopes", golden.length > 0, `${golden.length} envelopes`);
for (const envelope of golden) {
  let accepted = true;
  let reason = "";
  try {
    parseEnvelope(envelope);
  } catch (error) {
    accepted = false;
    reason = `${error}`;
  }
  check(`golden ${envelope.type} envelope is accepted`, accepted, reason);
}

// 4. The refusals the schema demands.
const request = golden.find((envelope) => envelope.type === "request");
check(
  "a field the schema does not allow is refused",
  !isEnvelope({ ...request, surprise: "value" }),
);
const withoutIdempotency = { ...request, meta: { ...request.meta } };
delete withoutIdempotency.meta.idempotencyKey;
check("a request without its idempotency key is refused", !isEnvelope(withoutIdempotency));
const event = golden.find((envelope) => envelope.type === "event");
const withoutCursor = { ...event };
delete withoutCursor.cursor;
check("an event without its cursor is refused", !isEnvelope(withoutCursor));
const errorEnvelope = golden.find((envelope) => envelope.type === "error");
const withoutRetryable = { ...errorEnvelope, error: { ...errorEnvelope.error } };
delete withoutRetryable.error.retryable;
check("an error body without retryable is refused", !isEnvelope(withoutRetryable));

if (failures.length > 0) {
  console.error(`\n${failures.length} check(s) failed`);
  process.exit(1);
}
console.log("\nall envelope checks passed");
