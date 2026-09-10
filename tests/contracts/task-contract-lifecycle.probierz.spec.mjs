import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { createReadStream, writeFileSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Cargo owns compilation. Execute its returned test artifact directly after
// signing; another Cargo invocation may restore an unsigned cached executable,
// and an unsigned `jeden-sandbox-helper` makes every real turn refuse with
// `enforced sandbox unavailable`.
//
// The suite to run is the first argument and defaults to `contracts`, so every
// real suite in this repository reaches its journeys through one signed path
// instead of a hand-run Cargo command.
const suite = (process.argv[2] || "contracts").trim();
const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const root = join(process.env.PROBIERZ_ARTIFACTS || join(repository, "target/contract-runs"), `runner-${suite}-${randomUUID()}`);
await mkdir(root, { recursive: true });
const tracePath = join(root, "trace.json");
const trace = { suite, status: "failed", sourceRevision: null, commands: [], binaries: [], completedAt: null };
process.once("exit", () => writeFileSync(tracePath, JSON.stringify(trace, null, 2)));
console.log(`Contract runner evidence: ${root}`);

async function command(argv) {
  const entry = { argv, cwd: repository, startedAt: new Date().toISOString(), stdout: "", stderr: "" };
  trace.commands.push(entry);
  await writeFile(tracePath, JSON.stringify(trace, null, 2));
  return new Promise((resolveCommand, reject) => {
    const child = spawn(argv[0], argv.slice(1), {
      cwd: repository, env: process.env, stdio: ["ignore", "pipe", "pipe"],
    });
    child.stdout.on("data", (chunk) => { entry.stdout += chunk.toString(); });
    child.stderr.on("data", (chunk) => { entry.stderr += chunk.toString(); process.stderr.write(chunk); });
    child.on("error", (error) => {
      entry.error = error.message;
      reject(error);
    });
    child.on("close", async (code, signal) => {
      Object.assign(entry, { exitCode: code, signal, completedAt: new Date().toISOString() });
      await writeFile(tracePath, JSON.stringify(trace, null, 2));
      resolveCommand(entry);
    });
  });
}

function succeeded(result) {
  assert.equal(result.exitCode, 0, `${result.argv.join(" ")}\n${result.stderr}`);
}

async function digest(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

try {
  const revision = await command(["git", "rev-parse", "HEAD"]);
  succeeded(revision);
  trace.sourceRevision = revision.stdout.trim();
  const patch = await command(["git", "diff", "--binary", "HEAD"]);
  succeeded(patch);
  await writeFile(join(root, "source.patch"), patch.stdout);
  const build = await command(["cargo", "test", "--locked", "--release", "--test", suite, "--no-run", "--message-format=json"]);
  succeeded(build);
  const artifacts = build.stdout.split("\n").filter(Boolean).map((line) => JSON.parse(line))
    .filter((event) => event.reason === "compiler-artifact" && event.executable);
  const tests = artifacts.filter((event) => event.target.name === suite && event.profile.test);
  assert.equal(tests.length, 1, `Cargo must return exactly the current ${suite} test executable`);
  const products = ["jeden", "jeden-sandbox-helper"].map((name) => {
    const artifact = artifacts.find((event) => event.target.name === name && !event.profile.test);
    assert.ok(artifact, `Cargo did not build ${name} for the real integration tests`);
    return artifact.executable;
  });
  if (process.platform === "darwin") {
    succeeded(await command(["wisent-products", "signing", "sign", "--product", "jeden", "--json", ...products]));
  }
  for (const path of [...products, tests[0].executable]) {
    trace.binaries.push({ path, sha256: await digest(path) });
  }
  const result = await command([tests[0].executable, "--nocapture"]);
  process.stdout.write(result.stdout);
  succeeded(result);
  trace.status = "passed";
  trace.completedAt = new Date().toISOString();
} catch (error) {
  trace.error = error.message;
  trace.completedAt = new Date().toISOString();
  process.exitCode = 1;
  console.error(error.message);
} finally {
  await writeFile(tracePath, JSON.stringify(trace, null, 2));
  if (process.env.PROBIERZ_MEDIA_MANIFEST) {
    await mkdir(dirname(process.env.PROBIERZ_MEDIA_MANIFEST), { recursive: true });
    await writeFile(process.env.PROBIERZ_MEDIA_MANIFEST,
      JSON.stringify([{ file: tracePath, kind: "trace", contentType: "application/json" }], null, 2));
  }
}
