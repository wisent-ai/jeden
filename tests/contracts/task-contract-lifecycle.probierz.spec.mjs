import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";
import { mkdir, mkdtemp, readFile, writeFile, access } from "node:fs/promises";
import { dirname, isAbsolute, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const artifacts = process.env.PROBIERZ_ARTIFACTS;
assert.ok(artifacts && isAbsolute(artifacts), "run this journey through Probierz");
const root = await mkdtemp(join(artifacts, "task-contract-"));
const home = join(root, "home");
const workspace = join(root, "workspace");
const sessions = join(root, "sessions");
const temporary = join(root, "temporary");
await Promise.all([home, workspace, sessions, temporary].map((path) => mkdir(path)));
const trace = {
  schemaVersion: 1,
  kind: "probierz-jeden-task-contract-lifecycle",
  status: "failed",
  completedAt: new Date().toISOString(),
  observation: { reply: "The task-contract lifecycle has not completed." },
  commands: [],
  observations: [],
  workspace,
  sessions,
};
const tracePath = join(root, "trace.json");
process.once("exit", (code) => {
  const exitCode = process.exitCode ?? code;
  trace.status = exitCode === 0 ? "completed" : "failed";
  trace.completedAt = new Date().toISOString();
  trace.observation.reply = `Jeden task-contract lifecycle exited with status ${exitCode}.`;
  writeFileSync(tracePath, JSON.stringify(trace, null, 2));
});
const environment = {
  ...process.env,
  HOME: home,
  JEDEN_SESSION_ROOT: sessions,
  TMPDIR: temporary,
  JEDEN_LANGUAGE: "en",
};
const binary = process.env.TUI_CMD;
assert.ok(binary && isAbsolute(binary), "TUI_CMD must name the source-bound Jeden binary");

async function retain() {
  trace.completedAt = new Date().toISOString();
  await writeFile(tracePath, JSON.stringify(trace, null, 2));
}

async function command(argv, { input = "", cwd = workspace, env = environment, timeout = 300000 } = {}) {
  const entry = { argv, cwd, startedAt: new Date().toISOString(), stdout: "", stderr: "" };
  trace.commands.push(entry);
  await retain();
  return new Promise((resolveCommand, reject) => {
    const child = spawn(argv[0], argv.slice(1), { cwd, env, stdio: ["pipe", "pipe", "pipe"] });
    let timedOut = false;
    const timer = setTimeout(() => { timedOut = true; child.kill("SIGKILL"); }, timeout);
    child.stdout.on("data", (data) => { entry.stdout += data.toString(); });
    child.stderr.on("data", (data) => { entry.stderr += data.toString(); });
    child.stdin.on("error", (error) => { entry.stdinError = error.message; });
    child.on("error", async (error) => {
      clearTimeout(timer);
      entry.error = error.message;
      await retain();
      reject(error);
    });
    child.on("close", async (code, signal) => {
      clearTimeout(timer);
      Object.assign(entry, { code, signal, timedOut, completedAt: new Date().toISOString() });
      await retain();
      resolveCommand(entry);
    });
    child.stdin.end(input);
  });
}

function succeeded(result) {
  assert.equal(result.timedOut, false, `${result.argv.join(" ")} timed out`);
  assert.equal(result.code, 0, `${result.argv.join(" ")}\n${result.stderr}\n${result.stdout}`);
}

async function settings() {
  return JSON.parse(await readFile(join(home, ".jeden", "config.yml"), "utf8"));
}


async function rpc(method, params) {
  const result = await command([binary, "rpc"], {
    input: `${JSON.stringify({ id: "contract", method, params })}\n${JSON.stringify({ id: "shutdown", method: "shutdown", params: {} })}\n`,
  });
  succeeded(result);
  const response = result.stdout.split("\n").filter((line) => line.trim()).map((line) => JSON.parse(line)).find((frame) => frame.id === "contract");
  assert.ok(response, "RPC must return the requested response");
  return response;
}

await test("Existing Rust operator-contract stories execute against the current product build", async () => {
  const result = await command([
    "cargo", "test", "--locked", "--release",
    "--test", "contracts", "operator_contracts_", "--", "--nocapture",
  ], {
    cwd: repository,
    env: { ...process.env, TMPDIR: temporary },
    timeout: 900000,
  });
  succeeded(result);
});

await test("CLI contract settings persist creation, editing and reset and reject unknown settings", async () => {
  for (const value of ["Use plain sentences.", "Answer in Polish."]) {
    succeeded(await command([binary, "config", "set", "contracts.communication", value]));
    assert.equal((await settings()).contracts.communication, value);
    const result = await command([binary, "config", "get", "contracts.communication"]);
    succeeded(result);
    assert.equal(result.stdout.trim(), value);
  }
  succeeded(await command([binary, "config", "set", "contracts.functionality", "Complete the requested operation."]));
  assert.equal((await settings()).contracts.functionality, "Complete the requested operation.");
  succeeded(await command([binary, "config", "reset", "contracts.functionality"]));
  assert.equal((await settings()).contracts.functionality, "");
  const before = await readFile(join(home, ".jeden", "config.yml"), "utf8");
  const refused = await command([binary, "config", "get", "contracts.style"]);
  assert.equal(refused.code, 1);
  assert.equal(refused.stderr.trim(), "Error: unknown config key: contracts.style");
  assert.equal(await readFile(join(home, ".jeden", "config.yml"), "utf8"), before);
  trace.observations.push({ operation: "config", persisted: await settings() });
  await retain();
});

await test("Desktop RPC exposes the same contract and persists edits visible through the CLI", async () => {
  const initial = await rpc("config/contracts/get", {});
  assert.equal(initial.error, undefined);
  const saved = await rpc("config/contracts/set", { communication: "Be concise.", functionality: "Finish the task." });
  assert.equal(saved.error, undefined);
  assert.equal((await settings()).contracts.communication, "Be concise.");
  assert.equal((await settings()).contracts.functionality, "Finish the task.");
  const result = await command([binary, "config", "get", "contracts.functionality"]);
  succeeded(result);
  assert.equal(result.stdout.trim(), "Finish the task.");
  const before = await readFile(join(home, ".jeden", "config.yml"), "utf8");
  const refused = await rpc("config/contracts/set", { communication: "Incomplete request." });
  assert.equal(refused.error.code, "invalid_params");
  assert.equal(refused.error.message, "functionality must be a string");
  assert.equal(await readFile(join(home, ".jeden", "config.yml"), "utf8"), before);
  trace.observations.push({ operation: "rpc", contract: initial.result.taskContract, persisted: await settings() });
  await retain();
});

async function modelTurn(task) {
  for (const name of ["BRAMA_URL", "BRAMA_TOKEN", "WISENT_APP_AGENT_ID", "WISENT_APP_AGENT_AUTH_SECRET", "JEDEN_MODEL"]) {
    assert.ok(environment[name], `${name} must be supplied by the real Brama workload configuration`);
  }
  const result = await command([binary, "run", task, "--json", "--allow-write", "--max-steps", "24"]);
  succeeded(result);
  const answer = JSON.parse(result.stdout.trim());
  assert.equal(answer.ok, true);
  assert.ok(answer.sessionPath.startsWith(`${sessions}${sep}`), "the turn must use its isolated session root");
  const events = (await readFile(join(answer.sessionPath, "transcript.jsonl"), "utf8")).split("\n").filter(Boolean).map((line) => JSON.parse(line)).map((event) => event.payload ?? event);
  const state = JSON.parse(await readFile(join(answer.sessionPath, "completion.json"), "utf8"));
  assert.equal(answer.completion.complete, true, "the native controller must accept the real result");
  assert.equal(answer.completion.status, "complete", "cancelled work is not verified implementation");
  assert.ok(state.requests.some((request) => request.prompt === task && request.coverageVerified));
  for (const item of state.tasks.filter((item) => item.status === "done")) {
    for (const evidence of item.verification.evidence) {
      const records = (await readFile(join(evidence.sessionPath, "transcript.jsonl"), "utf8"))
        .split("\n").filter(Boolean).map((line) => JSON.parse(line));
      assert.ok(records.some((event) => event.eventId === evidence.eventId), "verification must reference a real retained observation");
    }
  }
  trace.observations.push({ operation: "run", task, sessionPath: answer.sessionPath, completion: answer.completion });
  await retain();
  return events;
}

await test("Real Brama tasks create, edit and remove a file and retain complete reports", async () => {
  const instructions = "This is an explicitly requested isolated file-tool exercise, not new product development. Do not create software, documentation, tests or commits for it. Explain inapplicable delivery requirements honestly in the final structured report. ";
  await modelTurn(`${instructions}Create lifecycle.txt containing exactly alpha, using the real file tools, then read it back.`);
  assert.equal(await readFile(join(workspace, "lifecycle.txt"), "utf8"), "alpha");
  await modelTurn(`${instructions}Edit lifecycle.txt so its entire content is exactly beta, using the real file tools, then read it back.`);
  assert.equal(await readFile(join(workspace, "lifecycle.txt"), "utf8"), "beta");
  await modelTurn(`${instructions}Delete lifecycle.txt using the real file tools.`);
  await assert.rejects(access(join(workspace, "lifecycle.txt")), { code: "ENOENT" });
  trace.observations.push({ operation: "file-lifecycle", finalState: "removed" });
  await retain();
});

await test("A task that needs no tools still retains the complete report", async () => {
  const events = await modelTurn("Answer 2 + 2 without calling any tool, and provide the required structured delivery report with honest explanations of inapplicable requirements.");
  assert.equal(events.some((event) => event.type === "tool_call"), false);
});

await retain();
if (process.env.PROBIERZ_MEDIA_MANIFEST) {
  await mkdir(dirname(process.env.PROBIERZ_MEDIA_MANIFEST), { recursive: true });
  await writeFile(process.env.PROBIERZ_MEDIA_MANIFEST, JSON.stringify([{ file: tracePath, kind: "trace", contentType: "application/json" }], null, 2));
}
