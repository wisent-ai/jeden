// Exercise the real CLI and inspect its persisted model request and outcome.
// Declare the expected exit status and, for a refusal, its observed cause.
// A refusal journey does not qualify successful inference.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const required = name => {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} is required`);
  return value;
};
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = resolve(required('JEDEN_BIN'));
const revision = required('JEDEN_TEST_SOURCE_REVISION');
const model = required('JEDEN_TEST_MODEL');
const lakeBinary = resolve(required('JEDEN_TEST_LAKE_BIN'));
const lakeRevision = required('JEDEN_TEST_LAKE_REVISION');
const secretFormats = resolve(required('TRANSCRIPT_LAKE_SECRET_FORMATS'));
const expectedExit = Number(required('JEDEN_TEST_TRACKING_EXIT'));
assert.ok(Number.isSafeInteger(expectedExit));
const output = join(root, '.build', 'tracking', randomUUID());
const workspace = join(output, 'workspace');
const sessions = join(workspace, '.jeden', 'sessions');
mkdirSync(sessions, { recursive: true });
mkdirSync(workspace, { recursive: true });
const env = { ...process.env, JEDEN_SESSION_ROOT: sessions };
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const report = { revision, binary, binarySha256: hash(binary), journeySha256: hash(fileURLToPath(import.meta.url)),
  expectedExit, commands: [], exportFiles: [], verdict: 'failed' };
function invoke(program, args, cwd = workspace, environment = env) {
  const result = spawnSync(program, args, { cwd, env: environment, encoding: 'utf8' });
  report.commands.push({ program, args, cwd, status: result.status, signal: result.signal,
    stdout: result.stdout, stderr: result.stderr, error: result.error?.message });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  return result;
}
function succeeds(result) {
  assert.ok(result.status !== null && !result.status, result.stderr);
  return result.stdout;
}
function canonicalEvents(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return canonicalEvents(path);
    if (!entry.name.endsWith('.ndjson')) return [];
    return readFileSync(path, 'utf8').split('\n').filter(line => line.trim()).map(line => JSON.parse(line));
  });
}
try {
  assert.equal(succeeds(invoke('git', ['rev-parse', 'HEAD'], root)).trim(), revision);
  assert.equal(succeeds(invoke('git', ['status', '--porcelain'], root)).trim(), '',
    'qualify a committed candidate, not an unbound working-tree change');
  const marker = `Tracking observation at ${new Date().toISOString()}`;
  const prompt = `Respond exactly: ${marker}`;
  const result = invoke(binary, ['run', prompt, '--model', model, '--model-only', '--json']);
  assert.equal(result.status, expectedExit, `${result.stdout}\n${result.stderr}`);
  report.inferenceSucceeded = !result.status;
  if (result.status) {
    assert.ok(`${result.stdout}\n${result.stderr}`.includes(required('JEDEN_TEST_TRACKING_CAUSE')));
  } else {
    assert.ok(result.stdout.includes(marker), 'the real model did not return the requested text');
    const file = 'tracking-input.txt';
    const content = `Retained tool result at ${new Date().toISOString()}`;
    writeFileSync(join(workspace, file), content);
    const missing = 'missing-input.txt';
    report.toolCases = [];
    for (const target of [{ file, content }, { file: missing }]) {
      const userText = `Use read_file to read ${target.file}. Do not change files or run commands. Report the actual read result.`;
      const command = invoke(binary, ['run', userText,
        '--model', model, '--max-steps', required('JEDEN_TEST_MAX_STEPS'), '--json']);
      report.toolCases.push({ ...target, userText, commandStatus: command.status });
    }
  }
  const exports = readdirSync(sessions, { withFileTypes: true }).filter(entry => entry.isDirectory()).map(entry => {
    const session = join(sessions, entry.name);
    const path = join(output, `${entry.name}.json`);
    succeeds(invoke(binary, ['export', session, path]));
    report.exportFiles.push(path);
    return JSON.parse(readFileSync(path, 'utf8'));
  });
  const requests = exports.flatMap(session => session.events.filter(event =>
    event.type === 'interaction' && event.data?.type === 'operation_request' && event.data.operationKind === 'model'
    && event.data.input.messages.some(message => message.content === prompt)).map(event => ({ session, event })));
  assert.ok(requests.length, 'the original real model request was not persisted');
  for (const { session, event } of requests) {
    const outcomes = session.events.filter(candidate => candidate.type === 'interaction'
      && candidate.data?.type === 'operation_result' && candidate.data.requestEvent === event.id);
    assert.deepEqual(outcomes.map(candidate => candidate.data.outcome.status),
      [result.status ? 'failed' : 'completed'], 'each call needs its own exact terminal outcome');
    const [outcome] = outcomes;
    if (result.status) assert.ok(outcome.data.outcome.cause.includes(required('JEDEN_TEST_TRACKING_CAUSE')));
    else assert.ok(outcome.data.outcome.output.content.includes(marker));
  }
  if (report.toolCases) {
    const receipts = exports.flatMap(session => session.events.map(event => ({ session, event })));
    for (const target of report.toolCases) {
      const calls = receipts.filter(({ event }) => event.type === 'interaction'
        && event.data?.type === 'operation_request' && event.data.operationKind === 'tool' && event.data.operation === 'read_file'
        && JSON.stringify(event.data.input.arguments).includes(target.file));
      assert.ok(calls.length, `the real model never executed the requested read of ${target.file}`);
      for (const { session, event } of calls) {
        const terminal = session.events.filter(candidate => candidate.data?.type === 'operation_result'
          && candidate.data.requestEvent === event.id);
        assert.deepEqual(terminal.map(candidate => candidate.data.outcome.status),
          [target.content ? 'completed' : 'failed']);
        const [receipt] = terminal;
        if (target.content) assert.ok(JSON.stringify(receipt.data.outcome).includes(target.content),
          'the tool receipt lost the actual contents returned by the real file read');
      }
    }
    const events = exports.flatMap(session => session.events);
    assert.ok(events.some(event => event.data?.type === 'operation_request'
      && event.data.operationKind === 'model' && event.data.input.stream === false
      && events.some(result => result.data?.type === 'operation_result'
        && result.data.requestEvent === event.id && result.data.outcome.status === 'completed')),
    'the real execution did not persist a completed auxiliary buffered model call');
  }
  const refusedRoot = join(output, 'not-a-session-directory');
  writeFileSync(refusedRoot, 'an actual file, not a session directory');
  const refused = invoke(binary, ['run', prompt, '--model', model, '--model-only', '--json'],
    workspace, { ...env, JEDEN_SESSION_ROOT: refusedRoot });
  assert.ok(refused.status, 'an unwritable native ledger must refuse execution');
  assert.ok(`${refused.stdout}\n${refused.stderr}`.includes(refusedRoot),
    'the persistence refusal must name the actual failed path');
  const lake = join(output, 'lake');
  if (report.toolCases) {
    const cases = report.toolCases.map(target => {
      const session = exports.find(session => session.events.some(event =>
        event.data?.type === 'operation_request' && event.data.operationKind === 'tool'
        && event.data.operation === 'read_file'
        && JSON.stringify(event.data.input.arguments).includes(target.file)));
      assert.ok(session, `no genuine recorded session for ${target.file}`);
      const conclusion = session.events.find(event => event.data?.type === 'operation_result'
        && event.data.operationKind === 'model' && event.data.outcome.status === 'completed'
        && typeof event.data.outcome.output?.content === 'string'
        && event.data.outcome.output.content.includes(target.file));
      assert.ok(conclusion, `no genuine model response for ${target.file}`);
      const transcript = join(session.path, 'transcript.jsonl');
      return { sessionId: session.id, transcript, transcriptSha256: hash(transcript),
        userText: target.userText, file: target.file, expectedContent: target.content,
        refusedRead: !target.content, modelEventId: conclusion.id,
        modelContent: conclusion.data.outcome.output.content };
    });
    report.capture = join(output, 'tracking-capture.json');
    writeFileSync(report.capture, `${JSON.stringify({
      sourceHome: workspace, root: sessions, lakeRoot: lake, nativeRevision: revision,
      nativeBinarySha256: hash(binary), lakeRevision, lakeBinarySha256: hash(lakeBinary), cases,
    }, null, '\t')}\n`);
  }
  const lakeEnvironment = { ...env, HOME: workspace, TRANSCRIPT_LAKE_SECRET_FORMATS: secretFormats };
  succeeds(invoke(lakeBinary, ['--data-dir', lake, 'adopt', '--source', 'jeden', '--root', sessions, '--json'],
    workspace, lakeEnvironment));
  const canonical = canonicalEvents(join(lake, 'events')).filter(event => event.runtime === 'jeden');
  const metadata = canonical.filter(event => event.event_type === 'meta' && event.extra.kind === 'interaction');
  const original = exports.flatMap(session =>
    readFileSync(join(session.path, 'transcript.jsonl'), 'utf8').split('\n').filter(line => line.trim())
      .map(line => ({ session: session.id, event: JSON.parse(line) })))
    .filter(({ event }) => event.payload.type === 'interaction');
  assert.equal(metadata.length, original.length, 'archive ingestion dropped or duplicated actual execution records');
  for (const { session, event } of original.filter(({ event }) => event.payload.data.type === 'operation_request')) {
    const request = metadata.find(row => row.session_id === session && row.extra.sequence === event.sequence);
    assert.ok(request, 'the archive lost a native execution identity');
    const actual = metadata.filter(row => row.session_id === session
      && row.extra.requestEvent === request.extra.eventId);
    const expected = original.filter(source => source.session === session
      && source.event.payload.data.type === 'operation_result'
      && source.event.payload.data.requestEvent === event.eventId);
    assert.deepEqual(actual.map(row => row.extra.status),
      expected.map(source => source.event.payload.data.outcome.status),
      'the archive lost an outcome, confused a concurrent call or closed an unfinished request');
  }
  const expectedConclusion = result.status ? required('JEDEN_TEST_TRACKING_CAUSE') : marker;
  assert.ok(metadata.some(row => row.extra.operationKind === 'model'
    && row.extra.recordType === 'operation_result' && typeof row.text === 'string'
    && row.text.includes(expectedConclusion)),
  'the archive discarded the real model conclusion or actual refusal');
  if (report.toolCases) {
    const written = report.toolCases.find(target => target.content);
    assert.ok(metadata.some(row => row.extra.operationKind === 'tool'
      && row.extra.recordType === 'operation_result' && typeof row.text === 'string'
      && row.text.includes(written.content)),
    'the archive discarded the actual file contents returned by a tool');
  }
  const operatorTurns = exports.flatMap(session => session.events)
    .filter(event => event.type === 'user' && !event.data.modelOnly);
  assert.equal(canonical.filter(event => event.event_type === 'user').length, operatorTurns.length,
    'recording model requests or context snapshots fabricated operator instructions');
  report.archive = { binary: lakeBinary, revision: lakeRevision, binarySha256: hash(lakeBinary),
    root: lake, executionRecords: metadata.length, operatorTurns: operatorTurns.length };
  report.verdict = 'passed';
} catch (error) {
  report.error = String(error.stack || error);
  throw error;
} finally {
  writeFileSync(join(output, 'report.json'), `${JSON.stringify(report, null, '\t')}\n`);
  console.log(`${report.verdict}: ${join(output, 'report.json')}`);
}
