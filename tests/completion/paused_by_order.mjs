// Real journey: an order not to code pauses the retained work instead of
// forcing turn after turn. A real model is given work it cannot finish in its
// step budget, then, in the same session, the order to stop. Jeden's intake
// must pause every open task with the order quoted as its reason, the run
// must end on `Work remains paused` rather than continue, and a third request
// telling the work to go on must resume exactly those tasks.
// Inputs: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_MODEL, and the
// model route this Jeden is configured with (its own provider store or
// Brama). No build is performed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
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
function invoke(args) {
  const result = spawnSync(binary, args, { cwd: workspace, env, encoding: 'utf8' });
  report.commands.push({ args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  return result;
}
function tasks(session) {
  const listed = invoke(['todo', 'list', '--session', session, '--json']);
  assert.equal(listed.status, 0, listed.stderr);
  return JSON.parse(listed.stdout);
}

try {
  const git = (args) => spawnSync('git', args, { cwd: root, encoding: 'utf8' }).stdout.trim();
  report.checkoutRevision = git(['rev-parse', 'HEAD']);
  assert.equal(report.checkoutRevision, revision, 'run the journey from the declared candidate revision');
  assert.equal(git(['status', '--porcelain']), '', 'commit the candidate and tests before qualification');

  // One step cannot write three files, so the work is still open when the
  // order arrives.
  const work = 'Create three files a.txt, b.txt and c.txt in the working directory, each containing its own name.';
  const first = invoke(['run', work, '--cwd', workspace, '--model', model, '--max-steps', '1',
    '--allow-write', '--json']);
  report.first = first.status;
  const session = JSON.parse(readFileSync(join(workspace, '.jeden/mode-state.json'), 'utf8')).lastSessionPath;
  assert.ok(session, 'the workspace records no last session');
  report.session = session;
  const opened = tasks(session).tasks.filter((task) => !['done', 'cancelled'].includes(task.status));
  assert.ok(opened.length, 'the first run left no open task for the order to pause');

  const order = 'Nie koduj teraz, wstrzymaj tę pracę.';
  const stopped = invoke(['run', order, '--cwd', workspace, '--model', model, '--max-steps', '4',
    '--allow-write', '--resume-session', session, '--json']);
  assert.notEqual(stopped.status, 0, 'a run whose work is paused must not report success');
  assert.match(stopped.stderr, /Work remains paused/, stopped.stderr);
  const held = tasks(session);
  assert.equal(held.status, 'paused', JSON.stringify(held));
  for (const task of opened) {
    const now = held.tasks.find((each) => each.id === task.id);
    assert.equal(now.status, 'paused', `task ${task.id} was not paused by the order`);
    assert.ok(now.reason.includes(order), `task ${task.id} does not quote the order: ${now.reason}`);
  }

  const go = 'Wznów wstrzymaną pracę i dokończ ją.';
  invoke(['run', go, '--cwd', workspace, '--model', model, '--max-steps', '1', '--allow-write',
    '--resume-session', session, '--json']);
  const resumed = tasks(session);
  for (const task of opened) {
    const now = resumed.tasks.find((each) => each.id === task.id);
    assert.notEqual(now.status, 'paused', `task ${task.id} stayed paused after the order to go on`);
  }
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack);
  throw error;
} finally {
  report.finishedAt = new Date().toISOString();
  writeFileSync(join(run, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(run, 'report.json')}`);
}
