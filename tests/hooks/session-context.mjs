// Real Jeden RPC, Tama registry, Oko ledger and Brama route; no simulated provider.
// Run only when qualification is authorized. This file never builds a binary.
// Required inputs use the same JEDEN_TEST_* contract as answers.mjs, plus
// TAMA_OKO_CLI and JEDEN_TAMA_REGISTRY for the real installed integrations and
// JEDEN_TEST_COMPACTION_THRESHOLD for the automatic-compaction cases.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, closeSync, constants, mkdirSync, mkdtempSync, openSync, readFileSync, readdirSync, rmdirSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { exerciseMove } from '../sessions/move.mjs';
import { exerciseRead } from '../sessions/read.mjs';
import { exerciseMirror } from '../sessions/mirror.mjs';
import { exerciseAutoCompaction } from './compaction/journey.mjs';
import { exerciseHandoffRefusals } from './handoff/refusals.mjs';
import { startRpc } from './rpc.mjs';

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
function readRules(name) {
  const rules = JSON.parse(invoke(oko, ['tasks', 'rules', '--all', '--json']));
  assert.ok(Array.isArray(rules) && rules.some((rule) => rule.text?.trim()),
    'qualification requires actual standing instructions in Oko');
  writeFileSync(join(output, `${name}-rules.json`), JSON.stringify(rules));
  return rules;
}
let client;
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
  return snapshot;
}
try {
  report.checkoutRevision = invoke('git', ['rev-parse', 'HEAD'], root).trim();
  assert.equal(report.checkoutRevision, revision, 'candidate revision must match the checkout');
  assert.equal(invoke('git', ['status', '--porcelain'], root).trim(), '',
    'commit the candidate and tests before qualification');
  report.binaryVersion = invoke(binary, ['--version']);
  const startupRules = readRules('startup');
  client = startRpc({ binary, cwd, env, report });
  const { rpc, command } = client;
  const options = { cwd, model, maxSteps, allowCommand: true };
  const session = await rpc('session/new', { options });
  const prompt = (target, text) => rpc('session/prompt', { sessionId: target.sessionId, prompt: text });
  await prompt(session, 'Read the standing instructions. Do not edit files. What constraints govern your work?');
  const startupSnapshot = assertRuleSnapshot(exported(session.sessionPath, 'startup'), 'session_start', startupRules);
  let previousSnapshot = startupSnapshot;
  report.cases.push({ name: 'startup', verdict: 'passed', sessionPath: session.sessionPath,
    snapshotId: startupSnapshot.id });
  for (const name of ['first-compaction', 'repeated-compaction']) {
    const rules = readRules(name);
    await prompt(session, '/compact Preserve instructions, original requests and unfinished work.');
    const snapshot = assertRuleSnapshot(exported(session.sessionPath, name), 'compaction', rules);
    assert.notEqual(snapshot.id, previousSnapshot.id,
      `${name} reused an earlier model-context snapshot instead of restoring instructions again`);
    report.cases.push({ name, verdict: 'passed', sessionPath: session.sessionPath,
      snapshotId: snapshot.id, previousSnapshotId: previousSnapshot.id });
    previousSnapshot = snapshot;
  }
  const idle = await rpc('session/status', { sessionId: session.sessionId });
  assert.deepEqual(idle.activeRequestIds, [], 'finished maintenance left an active runtime request');
  const beforeDispose = hash(join(session.sessionPath, 'transcript.jsonl'));
  await rpc('session/dispose', { sessionId: session.sessionId });
  await assert.rejects(
    rpc('session/prompt', { sessionId: session.sessionId, prompt: 'This disposed session must not execute another turn.' }),
    (error) => error.message.includes('unknown_session') && error.message.includes(session.sessionId),
  );
  assert.equal(hash(join(session.sessionPath, 'transcript.jsonl')), beforeDispose,
    'a refused prompt changed the disposed session ledger');
  report.cases.push({ name: 'disposed-session-refuses-new-work', verdict: 'passed',
    sessionPath: session.sessionPath, transcriptSha256: beforeDispose });
  const resumeRules = readRules('resume');
  const reopened = await rpc('session/open', { session: session.sessionPath, options });
  await prompt(reopened, 'Read the standing instructions again without editing files.');
  const resumedSnapshot = assertRuleSnapshot(exported(reopened.sessionPath, 'resume'), 'session_start', resumeRules);
  assert.notEqual(resumedSnapshot.id, startupSnapshot.id,
    'resume reused the original startup snapshot instead of restoring instructions again');
  report.cases.push({ name: 'resume', verdict: 'passed', sessionPath: reopened.sessionPath,
    snapshotId: resumedSnapshot.id, previousStartupSnapshotId: startupSnapshot.id });
  const retainedBefore = await rpc('session/completion/get', { sessionId: reopened.sessionId });
  assert.ok(retainedBefore.completion.requests.length,
    'the real session must hold requests whose ownership a refused handoff must preserve');
  const sessionRoot = env.JEDEN_SESSION_ROOT;
  const originalMode = statSync(sessionRoot).mode;
  const unwritableMode = originalMode & ~(constants.S_IWUSR | constants.S_IWGRP | constants.S_IWOTH);
  const probe = join(sessionRoot, `permission-probe-${randomUUID()}`);
  let filesystemRefusal;
  const sessionsBeforeRefusal = readdirSync(sessionRoot).sort();
  try {
    chmodSync(sessionRoot, unwritableMode);
    try { mkdirSync(probe); } catch (error) { filesystemRefusal = error; }
    if (!filesystemRefusal) rmdirSync(probe);
    assert.ok(filesystemRefusal?.errno,
      'this host must enforce the actual directory write refusal before exercising handoff');
    report.filesystemRefusal = { code: filesystemRefusal.code, errno: filesystemRefusal.errno,
      message: filesystemRefusal.message, path: probe };
    const initializationRefusal = (message) => message.includes('cannot create ')
      && message.includes(sessionRoot) && message.includes('/artifacts')
      && message.includes(`os error ${Math.abs(filesystemRefusal.errno)}`);
    await assert.rejects(rpc('session/new', { options }),
      (error) => initializationRefusal(error.message));
    const args = ['run', 'Read the standing instructions.', '--model-only', '--cwd', cwd, '--model', model];
    const creation = spawnSync(binary, args, { cwd, env, encoding: 'utf8' });
    report.commands.push({ program: binary, args, cwd, status: creation.status,
      signal: creation.signal, stdout: creation.stdout, stderr: creation.stderr, error: creation.error?.message });
    assert.ifError(creation.error);
    assert.equal(creation.signal, null);
    assert.ok(creation.status !== null && creation.status);
    assert.ok(initializationRefusal(`${creation.stdout}\n${creation.stderr}`));
    assert.deepEqual(readdirSync(sessionRoot).sort(), sessionsBeforeRefusal,
      'CLI and RPC initialization refusals must not create a partial session');
    await assert.rejects(prompt(reopened, '/handoff Preserve all retained work and standing instructions.'),
      (error) => error.message.includes('handoff could not prepare child session')
        && error.message.includes(sessionRoot)
        && error.message.includes(`os error ${Math.abs(filesystemRefusal.errno)}`),
      'handoff must report its actual child-creation path and operating-system cause');
    await assert.rejects(rpc('session/open', { session: reopened.sessionPath, options }),
      (error) => error.message.includes('resume could not prepare child session')
        && error.message.includes(sessionRoot)
        && error.message.includes(`os error ${Math.abs(filesystemRefusal.errno)}`),
      'resume must report the same actual child-creation refusal without replacing active work');
    for (const [command, transition] of [['/clear', 'reset'], ['/fork', 'fork'], ['/branch preserved-work', 'branch']]) {
      await assert.rejects(prompt(reopened, command),
        (error) => error.message.includes(`${transition} could not prepare child session`)
          && error.message.includes(sessionRoot)
          && error.message.includes(`os error ${Math.abs(filesystemRefusal.errno)}`),
        `${transition} must preserve the active conversation on actual child-creation failure`);
    }
  } finally {
    chmodSync(sessionRoot, originalMode);
  }
  const retainedAfter = await rpc('session/completion/get', { sessionId: reopened.sessionId });
  for (const request of retainedBefore.completion.requests) {
    assert.deepEqual(retainedAfter.completion.requests.find((candidate) => candidate.id === request.id), request,
      'a refused child preparation must leave the active session owning every original request');
  }
  report.cases.push({ name: 'lifecycle-child-preparation-refusals', verdict: 'passed',
    sessionPath: reopened.sessionPath, retainedBefore, retainedAfter });
  await exerciseHandoffRefusals({ rpc, prompt, session: reopened, output, retainedBefore, report });
  const transcript = join(reopened.sessionPath, 'transcript.jsonl');
  const transcriptMode = statSync(transcript).mode;
  const beforeTranscript = hash(transcript);
  let appendRefusal;
  let appendProbe;
  const appendFailures = [];
  try {
    chmodSync(transcript, transcriptMode & ~(constants.S_IWUSR | constants.S_IWGRP | constants.S_IWOTH));
    try { appendProbe = openSync(transcript, 'a'); } catch (error) { appendRefusal = error; }
    assert.ok(appendRefusal?.errno, 'the host must enforce the actual transcript append refusal');
    await assert.rejects(prompt(reopened, '/checkpoint retained-context'),
      (error) => error.message.includes('cannot open transcript')
        && error.message.includes(transcript)
        && error.message.includes(`os error ${Math.abs(appendRefusal.errno)}`),
      'checkpoint persistence must report its actual append operation, file and operating-system cause');
  } catch (error) {
    appendFailures.push(error);
  } finally {
    try { chmodSync(transcript, transcriptMode); } catch (error) { appendFailures.push(error); }
    if (appendProbe !== undefined) {
      try { closeSync(appendProbe); } catch (error) { appendFailures.push(error); }
    }
  }
  if (appendFailures.length) {
    throw new AggregateError(appendFailures, `checkpoint qualification failed: ${appendFailures.map(String).join('; ')}`);
  }
  assert.equal(hash(transcript), beforeTranscript, 'a refused checkpoint must leave the durable transcript unchanged');
  report.cases.push({ name: 'checkpoint-append-refusal', verdict: 'passed', transcript,
    cause: appendRefusal.message, errno: appendRefusal.errno });
  await exerciseMove({ rpc, prompt, options, output, exported, report });
  await exerciseRead({ binary, env, rpc, prompt, options, exported, report });
  await exerciseMirror({ rpc, prompt, options, exported, report });
  await exerciseAutoCompaction({ binary, model, maxSteps, output, env, report, exported });
  // A genuinely missing executable exercises the native process refusal, not a mock response.
  const missing = join(output, `missing-hook-${randomUUID()}`);
  const quote = (text) => `'${text.replaceAll("'", "'\\''")}'`;
  writeFileSync(join(cwd, '.jeden/hooks.json'), JSON.stringify({ hooks: {
    SessionStart: [{ command: `exec ${quote(missing)}` }],
  } }));
  const refused = await rpc('session/new', { options });
  const isRestorationFailure = (error) =>
    error.message.includes('SessionStart (startup)') && error.message.includes(missing)
      && error.message.includes('exited');
  await assert.rejects(prompt(refused, 'Read the standing instructions.'), isRestorationFailure);
  await assert.rejects(prompt(refused, 'Continue after the failed instruction restoration.'), isRestorationFailure,
    'a failed restoration must remain pending on the next request');
  const failureEvents = exported(refused.sessionPath, 'refusal');
  assert.ok(!failureEvents.some((event) => event.type === 'action'),
    'a model action must not execute after the context hook fails');
  report.cases.push({ name: 'missing-executable-refusal', verdict: 'passed', sessionPath: refused.sessionPath });
  const failedTranscript = join(refused.sessionPath, 'transcript.jsonl');
  const beforeRetirement = hash(failedTranscript);
  const retirement = await rpc('session/retire', { sessionId: refused.sessionId });
  assert.deepEqual(retirement, { disposed: true }, 'failed prompt cleanup must leave the owning session retirable');
  await assert.rejects(rpc('session/status', { sessionId: refused.sessionId }),
    (error) => error.message.includes('unknown_session') && error.message.includes(refused.sessionId));
  assert.equal(hash(failedTranscript), beforeRetirement, 'idle retirement must preserve the actual failure ledger');
  report.cases.push({ name: 'idle-retirement-after-real-hook-refusal', verdict: 'passed',
    sessionPath: refused.sessionPath, transcriptSha256: beforeRetirement });
  await rpc('shutdown', {});
  await client.end();
  assert.ok(command.status !== null && !command.status, command.stderr);
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  try {
    if (client) await client.end();
  } catch (error) {
    report.verdict = 'failed';
    report.shutdownError = String(error);
    const cause = report.error
      ? `${report.error}; RPC shutdown also failed: ${error}`
      : `RPC shutdown failed: ${error}`;
    throw new Error(cause, { cause: error });
  } finally {
    report.finishedAt = new Date().toISOString();
    writeFileSync(join(output, 'report.json'), JSON.stringify(report));
    console.log(`${report.verdict}: ${join(output, 'report.json')}`);
  }
}
