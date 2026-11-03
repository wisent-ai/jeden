// Real journey: a turn that ends on a promise while its work is undone is not
// the end of the run. The real model is asked for a file and told to end its
// first turn with a promise to make it later; Jeden's completion authority
// must refuse that answer (completion_rejected), never publish it as final,
// and continue the same run until the file exists.
// Inputs: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_MODEL,
// JEDEN_TEST_MAX_STEPS, and the model route this Jeden is configured with
// (its own provider store or Brama). No build is performed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const model = required('JEDEN_TEST_MODEL');
const maxSteps = required('JEDEN_TEST_MAX_STEPS');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const run = join(root, '.build/completion-journeys', randomUUID());
const workspace = join(run, 'workspace');
const sessions = join(run, 'sessions');
for (const directory of [workspace, sessions]) mkdirSync(directory, { recursive: true });
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = {
  startedAt: new Date().toISOString(), binary, declaredSourceRevision: revision,
  binarySha256: hash(binary), testSha256: hash(fileURLToPath(import.meta.url)),
  model, commands: [], verdict: 'failed',
};
const env = { ...process.env, JEDEN_SESSION_ROOT: sessions };
function invoke(program, args, cwd) {
  const result = spawnSync(program, args, { cwd, env, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  return result;
}
function succeeds(result) {
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout;
}

try {
  report.checkoutRevision = succeeds(invoke('git', ['rev-parse', 'HEAD'], root)).trim();
  assert.equal(report.checkoutRevision, revision, 'run the journey from the declared candidate revision');
  assert.equal(succeeds(invoke('git', ['status', '--porcelain'], root)).trim(), '',
    'commit the candidate and tests before qualification');
  report.binaryVersion = succeeds(invoke(binary, ['--version'], root));

  const content = `delivered-${randomUUID()}`;
  const marker = `promise-${randomUUID()}`;
  const promise = `I will create result.txt later and let you know when it is done (${marker}).`;
  const task = `Create the file result.txt in the working directory containing exactly ${content}. `
    + 'This run qualifies how Jeden treats a turn that ends on a promise: before creating the file, '
    + `end your first turn with a final answer that is exactly "${promise}" and call no tool in it. `
    + 'When Jeden does not accept that answer, create the file and then finish.';
  const result = invoke(binary, ['run', task, '--cwd', workspace, '--model', model,
    '--max-steps', maxSteps, '--allow-write', '--json'], workspace);
  succeeds(result);

  const file = join(workspace, 'result.txt');
  assert.ok(existsSync(file), 'the run ended without the file the task asked for');
  assert.equal(readFileSync(file, 'utf8').trim(), content);

  const state = JSON.parse(readFileSync(join(workspace, '.jeden/mode-state.json'), 'utf8'));
  const session = state.lastSessionPath;
  assert.ok(session, 'the workspace records no last session');
  const exported = join(run, 'session-export.json');
  succeeds(invoke(binary, ['export', session, exported], workspace));
  const events = JSON.parse(readFileSync(exported, 'utf8')).events;
  report.session = session;

  const promised = events.findIndex((event) => event.type === 'action'
    && event.data.action?.action === 'final' && event.data.action.text?.includes(marker));
  assert.ok(promised !== -1, 'the real model must reach the promise this journey qualifies');
  const refused = events.findIndex((event, index) => index > promised
    && event.type === 'completion_rejected');
  assert.ok(refused !== -1, 'Jeden did not refuse the turn that ended on a promise');
  report.refusal = events[refused].data;
  assert.ok(!events.some((event) => event.type === 'final' && event.data.text?.includes(marker)),
    'the promise was published as the final answer');
  assert.ok(!`${result.stdout}\n${result.stderr}`.includes(marker), 'the promise reached the caller');
  const finals = events.filter((event, index) => index > refused && event.type === 'final');
  assert.ok(finals.length, 'the run did not continue to an accepted final answer');
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack);
  throw error;
} finally {
  report.finishedAt = new Date().toISOString();
  writeFileSync(join(run, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(run, 'report.json')}`);
}
