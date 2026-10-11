// Real journey: Oko's fleet runs a Jeden session as it runs OMP ones.
// `oko sessions start --harness jeden` starts Jeden in a broker terminal on a
// Brama lease Oko holds and hands it in JEDEN_LEASE; Jeden answers through
// Oko's proxy on that lease, records the route it was served on, `oko
// sessions agents` reads that model from Jeden's ledger, `oko sessions screen`
// reads the terminal and `oko sessions send` hands it a second prompt that
// Jeden answers. The terminal is ended at the close.
// Required: JEDEN_BIN, OKO_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_MODEL
// (a model `oko sessions start --model` takes). Waits follow the transcript's
// file events, not a clock.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, watch, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const jeden = resolve(required('JEDEN_BIN'));
const oko = resolve(required('OKO_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const model = required('JEDEN_TEST_MODEL');
assert.ok(!process.env.JEDEN_MODEL_ENDPOINT, 'the journey requires the real Brama route');
const runs = join(root, '.build/fleet-oko');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const workspace = join(output, 'workspace');
mkdirSync(workspace, { recursive: true });
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, jeden, jedenSha256: hash(jeden), oko, okoSha256: hash(oko), model,
  testSha256: hash(fileURLToPath(import.meta.url)), commands: [], observations: {}, verdict: 'failed' };

function invoke(program, args) {
  const result = spawnSync(program, args, { cwd: workspace, env: process.env, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.ok(result.status !== null && !result.status, `${program} ${args.join(' ')}: ${result.stderr}`);
  return result.stdout;
}
function events(transcript) {
  return readFileSync(transcript, 'utf8').split('\n').filter(Boolean).map((line) => JSON.parse(line).payload);
}
// Resolves once `holds` is true of the transcript's events, re-reading it on
// every change the kernel reports for it.
function until(transcript, holds) {
  return new Promise((resolvePromise, reject) => {
    const check = () => {
      try {
        const seen = events(transcript);
        if (holds(seen)) {
          watcher.close();
          resolvePromise(seen);
        }
      } catch (error) {
        watcher.close();
        reject(error);
      }
    };
    const watcher = watch(transcript, check);
    check();
  });
}

let terminal;
try {
  assert.equal(invoke('git', ['-C', root, 'rev-parse', 'HEAD']).trim(), revision);
  const started = JSON.parse(invoke(oko, ['sessions', 'start', '--harness', 'jeden', '--model', model,
    '--text', 'Respond exactly: FIRST', '--cwd', workspace, '--json']));
  report.observations.started = started;
  terminal = started.id;
  assert.ok(started.session && started.transcript && terminal, 'oko sessions start names the session, transcript and terminal');

  const first = await until(started.transcript, (seen) => seen.some((event) => event.type === 'model_route'));
  const route = first.find((event) => event.type === 'model_route').data.model;
  const lease = first.find((event) => event.type === 'model_subscription_route');
  report.observations.firstRoute = route;
  report.observations.lease = lease?.data;
  assert.ok(lease, 'the step must run on the lease Oko handed the session');

  const agents = JSON.parse(invoke(oko, ['sessions', 'agents', '--json']));
  const row = agents.sessions.find((entry) => entry.session === started.session);
  report.observations.agentsRow = row;
  assert.ok(row, 'oko sessions agents lists the Jeden session');
  assert.equal(row.model, route, 'oko reads the model the Jeden session was served on');

  const screen = JSON.parse(invoke(oko, ['sessions', 'screen', terminal, '--json'])).screen;
  report.observations.screen = screen;
  assert.ok(screen.includes('FIRST'), 'the terminal screen shows the first answer');

  invoke(oko, ['sessions', 'send', terminal, '--text', 'Respond exactly: SECOND\r']);
  const second = await until(started.transcript, (seen) => {
    const asked = seen.find((event) => event.type === 'user' && JSON.stringify(event.data).includes('SECOND'));
    return Boolean(asked) && seen.slice(seen.indexOf(asked)).some((event) => event.type === 'model_route');
  });
  report.observations.secondEvents = second.length;
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  if (terminal) {
    const ended = spawnSync(oko, ['sessions', 'end', terminal, '--json'], { encoding: 'utf8' });
    report.commands.push({ program: oko, args: ['sessions', 'end', terminal, '--json'], status: ended.status,
      stdout: ended.stdout, stderr: ended.stderr, error: ended.error?.message });
  }
  writeFileSync(join(output, 'report.json'), JSON.stringify(report));
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
