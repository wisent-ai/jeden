// Real import and Oko inventory journey over a retained harness transcript.
// No conversation, dependency executable or provider answer is manufactured.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFileSync, cpSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { filesystemJourneys } from './filesystem.mjs';
import { completionJourneys } from './completion.mjs';
import { bindJedenExecutable, bindOkoExecutable } from './identity.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required for the actual product journey`);
  return value;
}
const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const jeden = resolve(required('JEDEN_BIN'));
const oko = resolve(required('OKO_BIN'));
const source = realpathSync(required('JEDEN_TEST_IMPORT_SOURCE'));
const retainedSource = realpathSync(required('JEDEN_TEST_RETAINED_SESSION'));
const jedenRevision = required('JEDEN_TEST_SOURCE_REVISION');
const okoRepository = resolve(required('OKO_TEST_SOURCE_ROOT'));
const okoRevision = required('OKO_TEST_SOURCE_REVISION');
const runs = join(repository, '.build/import-workspace');
mkdirSync(runs, { recursive: true });
const output = mkdtempSync(join(runs, 'run-'));
const home = join(output, 'home');
mkdirSync(home, { recursive: true });
const sessionRoot = join(home, '.jeden/sessions');
const digest = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = {
  jedenRevision, okoRevision, jeden, oko, source,
  jedenSha256: digest(jeden), okoSha256: digest(oko), sourceSha256: digest(source),
  commands: [], observations: {}, filesystemObservations: {}, verdict: 'failed',
};
const reportPath = join(output, 'report.json');
const retain = (initiating) => {
  try {
    writeFileSync(reportPath, JSON.stringify(report));
  } catch (publication) {
    if (initiating === undefined) throw publication;
    throw new AggregateError([initiating, publication],
      `Cannot retain import qualification report ${reportPath}; initiating failure:\n${String(initiating.stack || initiating)}\nreport failure:\n${String(publication.stack || publication)}`);
  }
};
function invoke(program, args, environment = process.env) {
  const result = spawnSync(program, args, { cwd: repository, env: environment, encoding: 'utf8' });
  report.commands.push({ program, args, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  retain(result.error);
  assert.ifError(result.error);
  return result;
}
function success(result) {
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  return result.stdout;
}
const environment = { ...process.env, HOME: home, JEDEN_SESSION_ROOT: sessionRoot,
  TAMA_SESSION_CONTROL_ROOT: join(home, 'tama-sessions') };
delete environment.PROBIERZ_INTAKE_URL;
delete environment.PROBIERZ_INTAKE_TOKEN;
function retainedWorkspace() {
  const sourceState = join(retainedSource, 'state.json');
  const sourceTranscript = join(retainedSource, 'transcript.jsonl');
  const state = JSON.parse(readFileSync(sourceState, 'utf8'));
  assert.equal(state.id, basename(retainedSource), 'the fixture is a real native ledger directory');
  assert.ok(typeof state.cwd === 'string' && isAbsolute(state.cwd));
  const originalState = digest(sourceState);
  const originalTranscript = digest(sourceTranscript);
  const events = readFileSync(sourceTranscript, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
  assert.ok(events.some((event) => event.payload?.type === 'context_snapshot'));
  for (const event of events) {
    assert.equal(event.payload?.data?.cwd, undefined,
      'the genuine retained fixture must reproduce a pre-workspace native transcript');
  }
  report.retainedCapture = { source: retainedSource, stateSha256: originalState,
    transcriptSha256: originalTranscript, session: state.id, cwd: state.cwd };
  retain();
  const isolatedHome = join(output, 'retained-home');
  const root = join(isolatedHome, '.jeden/sessions');
  mkdirSync(root, { recursive: true });
  const copy = join(root, state.id);
  cpSync(retainedSource, copy, { recursive: true, force: false, errorOnExist: true });
  const copiedState = join(copy, 'state.json');
  const copiedTranscript = join(copy, 'transcript.jsonl');
  assert.equal(digest(copiedState), originalState);
  assert.equal(digest(copiedTranscript), originalTranscript);
  const scoped = { ...environment, HOME: isolatedHome, JEDEN_SESSION_ROOT: root,
    TAMA_SESSION_CONTROL_ROOT: join(isolatedHome, 'tama-sessions') };
  function inventory() {
    const observed = JSON.parse(success(invoke(oko, ['sessions', 'agents', '--json'], scoped)));
    const row = observed.sessions.find((entry) => entry.session === state.id);
    assert.ok(row, 'the actual inventory must retain even an unreadable native ledger');
    return row;
  }
  const before = inventory();
  assert.equal(before.cwd, state.cwd);
  assert.ok(!before.readFailure, JSON.stringify(before));
  writeFileSync(copiedState, '{');
  const corrupt = inventory();
  assert.ok(corrupt.readFailure.includes(copiedState), JSON.stringify(corrupt));
  assert.ok(corrupt.cwd === null || corrupt.cwd === undefined);
  rmSync(copiedState);
  const missing = inventory();
  assert.ok(missing.readFailure.includes(copiedState), JSON.stringify(missing));
  assert.notEqual(missing.readFailure, corrupt.readFailure,
    'a missing artifact and a JSON decode failure must retain their distinct actual causes');
  cpSync(sourceState, copiedState, { force: false, errorOnExist: true });
  const restored = inventory();
  assert.equal(restored.cwd, state.cwd);
  assert.ok(!restored.readFailure, JSON.stringify(restored));
  appendFileSync(copiedTranscript, '{');
  const incomplete = inventory();
  assert.ok(incomplete.modelFailure.includes(copiedTranscript), JSON.stringify(incomplete));
  cpSync(sourceTranscript, copiedTranscript);
  assert.equal(digest(copiedTranscript), originalTranscript);
  assert.equal(digest(sourceState), originalState, 'the operator ledger state remains untouched');
  assert.equal(digest(sourceTranscript), originalTranscript, 'the operator history remains untouched');
  report.retainedObservations = { before, corrupt, missing, restored, incomplete };
  retain();
}
let initiating;
try {
  const qualification = { output, jeden, oko, environment, invoke, success, report, retain, digest };
  bindJedenExecutable(qualification, repository, jedenRevision);
  bindOkoExecutable(qualification, okoRepository, okoRevision);
  retainedWorkspace();
  const imported = JSON.parse(success(invoke(jeden, ['import', source, '--json'], environment)));
  assert.deepEqual(imported.failures, [], JSON.stringify(imported));
  const [session, ...others] = imported.sessions;
  assert.ok(session, 'the actual importer names its persisted ledger');
  assert.deepEqual(others, []);
  assert.ok(typeof session.cwd === 'string' && isAbsolute(session.cwd),
    'the captured source must identify an absolute workspace, not matching absent fields');
  const transcript = join(session.sessionPath, 'transcript.jsonl');
  const state = JSON.parse(readFileSync(join(session.sessionPath, 'state.json'), 'utf8'));
  const events = readFileSync(transcript, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
  const context = events.find((event) => event.payload?.type === 'context_snapshot');
  assert.ok(context, 'the actual import persists the source history');
  assert.equal(context.payload.data.cwd, session.cwd);
  assert.equal(state.cwd, session.cwd);
  filesystemJourneys({ output, jeden, source, environment, invoke, success, report, retain, digest }, session);
  completionJourneys({ output, jeden, source, environment, invoke, success, report, retain, digest });
  const observed = JSON.parse(success(invoke(oko, ['sessions', 'agents', '--json'], environment)));
  const row = observed.sessions.find((entry) => entry.session === state.id);
  assert.ok(row, 'Oko observes the real imported native ledger before any model turn');
  assert.equal(row.cwd, state.cwd, JSON.stringify(row));
  report.observations = { import: imported, state, inventory: row };
  const before = digest(transcript);
  const existing = JSON.parse(success(invoke(jeden, ['import', source, '--json'], environment)));
  assert.deepEqual(existing.failures, []);
  assert.equal(existing.sessions.at(0)?.sessionPath, session.sessionPath);
  assert.equal(digest(transcript), before, 'an unchanged import preserves its native transcript');
  const refused = invoke(jeden, ['import', source, '--unsupported-option', '--json'], environment);
  // https://pubs.opengroup.org/onlinepubs/9799919799/utilities/V3_chap02.html#tag_19_08_02
  assert.notEqual(refused.status, 0);
  assert.equal(refused.signal, null, 'termination by a signal is not a product refusal');
  assert.match(refused.stderr, /unsupported-option/);
  assert.equal(digest(transcript), before, 'a usage refusal does not replace retained history');
  const missing = join(output, 'missing-transcript.jsonl');
  const absent = JSON.parse(success(invoke(jeden, ['import', missing, '--json'], environment)));
  assert.deepEqual(absent.sessions, []);
  assert.ok(absent.failures.some((failure) => failure.sourcePath === missing && failure.error.includes(missing)),
    JSON.stringify(absent));
  assert.equal(digest(transcript), before);
  assert.equal(digest(source), report.sourceSha256, 'the operator source is never rewritten');
  report.verdict = 'passed';
} catch (error) {
  initiating = error;
  report.error = String(error.stack || error);
  throw error;
} finally {
  retain(initiating);
  console.log(`${report.verdict}: ${reportPath}`);
}
