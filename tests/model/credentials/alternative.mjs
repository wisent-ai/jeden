// Real journey: with no Brama address exported, Jeden hands each model
// request to Stado's inference alternatives (`stado capabilities call
// inference --consumer jeden`) and names none of them itself. In a home whose
// registry places Brama nowhere, with the real Stado JEDEN_TEST_STADO first
// on PATH as `stado`:
//   1. `jeden doctor --json` reports the route as Stado's alternatives;
//   2. `jeden run` is refused with Stado's own sentence, which names every
//      alternative it tried (brama, attestation) with its cause and the
//      command that answers the kept request;
//   3. that refusal stays in the durable session `jeden export` writes;
//   4. the operator answers that exact request with `stado capabilities
//      attestation record`, and the same turn is then served by that
//      statement: Jeden prints it, with no Brama, no credential and no
//      endpoint declared;
//   5. the graphical clients' interface (`jeden rpc`) gets Stado's sentence
//      both as the prompt's error and as its error notification.
// Inputs: JEDEN_BIN, JEDEN_TEST_SOURCE_REVISION, JEDEN_TEST_STADO,
// JEDEN_TEST_MODEL. No simulated provider: the answer is the operator's
// recorded statement, read back through the real Stado.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, readdirSync, symlinkSync, writeFileSync } from 'node:fs';
import { basename, join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { commands } from './commands.mjs';
import { startRpc } from '../../hooks/rpc.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const area = join(root, '.build/model-alternative', randomUUID());
const home = join(area, 'home');
const bin = join(area, 'bin');
const workspace = join(area, 'workspace');
const sessions = join(area, 'sessions');
for (const directory of [home, bin, workspace, sessions, join(area, 'tmp'), join(area, 'storage')]) {
  mkdirSync(directory, { recursive: true });
}
const hash = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { testSha256: hash(fileURLToPath(import.meta.url)), commands: [], frames: [], verdict: 'failed' };
const transcripts = (directory) => readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
  const path = join(directory, entry.name);
  return entry.isDirectory() ? transcripts(path) : entry.name === 'transcript.jsonl' ? [path] : [];
});
const unanswered = 'no inference alternative answers jeden';
const required = (name) => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must name a real alternative journey input`);
  return value;
};
const { invoke, succeeds, refuses } = commands({ root, area, report, redacted: (value) => value });

try {
  const binary = resolve(required('JEDEN_BIN'));
  const revision = required('JEDEN_TEST_SOURCE_REVISION');
  const stado = resolve(required('JEDEN_TEST_STADO'));
  const model = required('JEDEN_TEST_MODEL');
  assert.equal(basename(stado), 'stado', `${stado} is not a stado binary`);
  report.revision = revision;
  report.binarySha256 = hash(binary);
  report.stadoSha256 = hash(stado);
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'], process.env)).trim(), revision);
  symlinkSync(stado, join(bin, 'stado'));
  report.stadoVersion = succeeds(invoke(stado, ['--version'], process.env)).trim();

  // The candidate Stado leads PATH; the rest of the caller's PATH stays,
  // because Jeden's extension host runs on it. No Brama address, secret or
  // provider is in this environment.
  const env = {
    PATH: `${bin}:${process.env.PATH}`, HOME: home, TMPDIR: join(area, 'tmp'),
    WC_STORAGE_BACKEND: 'local', WC_LOCAL_STORAGE_PATH: join(area, 'storage'),
    JEDEN_SESSION_ROOT: sessions,
  };
  const stadoRun = (args, input) => succeeds(invoke(stado, args, env, input));
  stadoRun(['config', 'init']);

  // 1. Doctor names the route Jeden takes. Doctor ends nonzero whenever any
  //    subsystem of this bare home is unavailable, so its report is read
  //    from stdout and only the model route's probe is judged.
  const doctor = JSON.parse(invoke(binary, ['doctor', '--json'], env).stdout);
  const probe = doctor.probes.find((entry) => entry.subsystem === 'brama');
  assert.equal(probe?.state, 'degraded', JSON.stringify(probe));
  assert.ok(probe.detail.includes("Stado's inference alternatives"), probe.detail);
  assert.ok(probe.detail.includes('stado capabilities call inference --consumer jeden'), probe.detail);
  assert.deepEqual(probe.evidence.order, ['brama', 'attestation'], JSON.stringify(probe));

  // 2. Nothing answers yet: Stado's own sentence reaches the operator.
  const turn = ['run', 'Respond exactly: OK', '--model-only', '--cwd', workspace, '--model', model];
  const refused = invoke(binary, turn, env);
  refuses(refused, unanswered);
  refuses(refused, 'brama at ');
  refuses(refused, 'attestation at ');
  refuses(refused, 'stado capabilities attestation record inference --consumer jeden');

  // 3. The refusal stays in the durable session.
  const events = transcripts(sessions).flatMap((path) => {
    const exported = join(dirname(path), 'alternative-export.json');
    succeeds(invoke(binary, ['export', dirname(path), exported], env));
    return JSON.parse(readFileSync(exported, 'utf8')).events;
  });
  assert.ok(events.some((event) => event.type === 'run_error' && event.data?.message?.includes(unanswered)),
    'the refusal must remain in the durable session');
  assert.ok(!events.some((event) => event.type === 'final'), 'a refused turn must not manufacture completion');

  // 4. The operator answers that exact request; the same turn is served by it.
  const kept = JSON.parse(stadoRun(['capabilities', 'attestation', 'list', 'inference', '--consumer', 'jeden', '--json']));
  const open = kept.filter((entry) => !entry.attested);
  assert.equal(open.length, 1, `exactly the refused turn must be kept: ${JSON.stringify(kept)}`);
  const request = open[0].request;
  const shown = JSON.parse(stadoRun(['capabilities', 'attestation', 'show', 'inference', '--consumer', 'jeden', request]));
  assert.ok(JSON.stringify(shown.asked).includes('Respond exactly: OK'), JSON.stringify(shown));
  const statement = `attested-${randomUUID()}`;
  stadoRun(['capabilities', 'attestation', 'record', 'inference', '--consumer', 'jeden', request,
    '--checked', 'the alternatives journey states this answer itself'], statement);
  const served = invoke(binary, turn, env);
  assert.ok(succeeds(served).includes(statement), `the attested answer did not serve the turn: ${served.stdout}`);
  report.request = request;

  // 5. The graphical clients' interface carries Stado's sentence.
  const client = startRpc({ binary, cwd: workspace, env, report });
  try {
    const session = await client.rpc('session/new', { options: { cwd: workspace, model, maxSteps: 1 } });
    const requestId = randomUUID();
    let rpcRefused;
    try {
      await client.rpc('session/prompt', { sessionId: session.sessionId, requestId, prompt: `Respond exactly: ${randomUUID()}` });
    } catch (error) {
      rpcRefused = error;
    }
    assert.ok(rpcRefused instanceof Error, 'the graphical-client turn nothing answers must be refused');
    const notification = report.frames
      .filter(({ frame }) => frame.method === 'session/event' && frame.params?.requestId === requestId)
      .map(({ frame }) => frame.params)
      .find((event) => event.kind === 'error');
    assert.ok(notification, 'the graphical client must receive the refusal notification');
    for (const text of [rpcRefused.message, notification.message]) {
      assert.ok(text.includes(unanswered), text);
      assert.ok(text.includes('stado capabilities attestation record inference --consumer jeden'), text);
    }
    await client.rpc('shutdown', {});
  } finally {
    const ended = await client.end();
    assert.ok(ended.status !== null && !ended.status, `jeden rpc did not shut down: ${JSON.stringify(ended)}`);
  }
  report.verdict = 'passed';
} finally {
  writeFileSync(join(area, 'report.json'), JSON.stringify(report, null, '  '));
  console.log(`${report.verdict}: ${join(area, 'report.json')}`);
}
