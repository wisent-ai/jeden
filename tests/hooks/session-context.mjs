// Real Jeden RPC, Tama registry, Oko ledger and Brama route; no simulated provider.
// Run only when qualification is authorized. This file never builds a binary.
// Required inputs use the same JEDEN_TEST_* contract as answers.mjs, plus
// TAMA_OKO_CLI and JEDEN_TAMA_REGISTRY for the real installed integrations.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const model = required('JEDEN_TEST_MODEL');
const maxSteps = Number(required('JEDEN_TEST_MAX_STEPS'));
assert.ok(Number.isSafeInteger(maxSteps), 'JEDEN_TEST_MAX_STEPS must be an integer');
const oko = required('TAMA_OKO_CLI');
const registry = resolve(required('JEDEN_TAMA_REGISTRY'));
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const runs = join(root, '.build/session-context-journeys');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const cwd = join(output, 'workspace');
mkdirSync(join(cwd, '.jeden'), { recursive: true });
const env = { ...process.env, JEDEN_SESSION_ROOT: join(output, 'sessions') };
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { startedAt: new Date().toISOString(), binary, declaredSourceRevision: revision,
  binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  registry, registrySha256: hash(registry), commands: [], frames: [], cases: [], verdict: 'failed' };
function invoke(program, args, directory = cwd) {
  const result = spawnSync(program, args, { cwd: directory, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd: directory, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout;
}
let child;
let exited;
const pending = new Map();
let processFailure;
function fail(error) {
  processFailure = error;
  for (const request of pending.values()) request.reject(error);
  pending.clear();
}
function rpc(method, params) {
  if (processFailure) return Promise.reject(processFailure);
  const id = randomUUID();
  const frame = { id, method, params };
  report.frames.push({ direction: 'request', frame });
  return new Promise((resolveRequest, reject) => {
    pending.set(id, { resolve: resolveRequest, reject });
    child.stdin.write(`${JSON.stringify(frame)}\n`, (error) => {
      if (error) fail(error);
    });
  });
}
function exported(sessionPath, name) {
  const text = invoke(binary, ['export', sessionPath]);
  writeFileSync(join(output, `${name}.json`), text);
  const value = JSON.parse(text);
  assert.ok(Array.isArray(value.events), 'export must contain the real ledger');
  return value.events;
}
function assertRuleSnapshot(events, reason, rules) {
  const snapshot = events.findLast((event) => event.type === 'context_snapshot'
    && event.data.reason === reason);
  assert.ok(snapshot, `missing ${reason} model-context snapshot`);
  const system = snapshot.data.messages.filter((message) => message.role === 'system')
    .map((message) => message.content).join('\n');
  for (const rule of rules) assert.ok(system.includes(rule.text),
    `lost complete rule ${rule.sessionId} #${rule.ordinal} after ${reason}`);
}
try {
  report.checkoutRevision = invoke('git', ['rev-parse', 'HEAD'], root).trim();
  assert.equal(report.checkoutRevision, revision, 'candidate revision must match the checkout');
  assert.equal(invoke('git', ['status', '--porcelain'], root).trim(), '',
    'commit the candidate and tests before qualification');
  report.binaryVersion = invoke(binary, ['--version']);
  const rules = JSON.parse(invoke(oko, ['tasks', 'rules', '--all', '--json']));
  assert.ok(Array.isArray(rules) && rules.some((rule) => rule.text?.trim()),
    'qualification requires actual standing instructions in Oko');
  writeFileSync(join(output, 'rules.json'), JSON.stringify(rules));
  child = spawn(binary, ['rpc'], { cwd, env, stdio: ['pipe', 'pipe', 'pipe'] });
  const command = { program: binary, args: ['rpc'], cwd, stderr: '' };
  report.commands.push(command);
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (text) => { command.stderr += text; });
  child.on('error', fail);
  exited = new Promise((resolveExit) => child.on('close', (status, signal) => {
    Object.assign(command, { status, signal });
    fail(new Error(`RPC process ended: status=${status}, signal=${signal}; ${command.stderr}`));
    resolveExit();
  }));
  createInterface({ input: child.stdout }).on('line', (line) => {
    try {
      const frame = JSON.parse(line);
      report.frames.push({ direction: 'response', frame });
      const request = pending.get(frame.id);
      if (!request) return;
      pending.delete(frame.id);
      if (frame.error) request.reject(new Error(`${frame.error.code}: ${frame.error.message}`));
      else request.resolve(frame.result);
    } catch (error) { fail(error); }
  });
  const options = { cwd, model, maxSteps, allowCommand: true };
  const session = await rpc('session/new', { options });
  const prompt = (target, text) => rpc('session/prompt', { sessionId: target.sessionId, prompt: text });
  await prompt(session, 'Read the standing instructions. Do not edit files. What constraints govern your work?');
  assertRuleSnapshot(exported(session.sessionPath, 'startup'), 'session_start', rules);
  report.cases.push({ name: 'startup', verdict: 'passed', sessionPath: session.sessionPath });
  for (const name of ['first-compaction', 'repeated-compaction']) {
    await prompt(session, '/compact Preserve instructions, original requests and unfinished work.');
    assertRuleSnapshot(exported(session.sessionPath, name), 'compaction', rules);
    report.cases.push({ name, verdict: 'passed', sessionPath: session.sessionPath });
  }
  await rpc('session/dispose', { sessionId: session.sessionId });
  const reopened = await rpc('session/open', { session: session.sessionPath, options });
  await prompt(reopened, 'Read the standing instructions again without editing files.');
  assertRuleSnapshot(exported(reopened.sessionPath, 'resume'), 'session_start', rules);
  report.cases.push({ name: 'resume', verdict: 'passed', sessionPath: reopened.sessionPath });
  // A genuinely missing executable exercises the native process refusal, not a mock response.
  const missing = join(output, `missing-hook-${randomUUID()}`);
  const quote = (text) => `'${text.replaceAll("'", "'\\''")}'`;
  writeFileSync(join(cwd, '.jeden/hooks.json'), JSON.stringify({ hooks: {
    SessionStart: [{ command: `exec ${quote(missing)}` }],
  } }));
  const refused = await rpc('session/new', { options });
  await assert.rejects(prompt(refused, 'Read the standing instructions.'), (error) =>
    error.message.includes('SessionStart (startup)') && error.message.includes(missing)
      && error.message.includes('exited'));
  const failureEvents = exported(refused.sessionPath, 'refusal');
  assert.ok(!failureEvents.some((event) => event.type === 'action'),
    'a model action must not execute after the context hook fails');
  report.cases.push({ name: 'missing-executable-refusal', verdict: 'passed', sessionPath: refused.sessionPath });
  await rpc('shutdown', {});
  child.stdin.end();
  await exited;
  assert.ok(command.status !== null && !command.status, command.stderr);
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  if (child && child.exitCode === null && child.signalCode === null) {
    child.stdin.end();
    await exited;
  }
  report.finishedAt = new Date().toISOString();
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
