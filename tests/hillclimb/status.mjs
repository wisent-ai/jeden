// The real RPC process must read the workspace the caller named, never its
// process directory when cwd was omitted or blank. No model call is needed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRpc } from '../hooks/rpc.mjs';

function required(name) {
  const value = process.env[name];
  assert.ok(value?.trim(), `${name} must be declared`);
  return value;
}
const root = fileURLToPath(new URL('../../', import.meta.url));
const binary = resolve(required('JEDEN_BIN'));
const expectedRevision = required('JEDEN_TEST_SOURCE_REVISION');
const output = join(root, '.build', `hillclimb-status-${randomUUID()}`);
const home = join(output, 'home');
const firstWorkspace = join(output, 'first');
const secondWorkspace = join(output, 'second');
for (const directory of [home, firstWorkspace, secondWorkspace]) mkdirSync(directory, { recursive: true });
const report = {
  binary,
  binarySha256: createHash('sha256').update(readFileSync(binary)).digest('hex'),
  sourceRevision: expectedRevision,
  commands: [], frames: [], verdict: 'failed', startedAt: new Date().toISOString(),
};
function run(program, args) {
  const result = spawnSync(program, args, { cwd: root, encoding: 'utf8' });
  report.commands.push({ program, args, cwd: root, status: result.status, signal: result.signal,
    error: result.error?.stack, stdout: result.stdout, stderr: result.stderr });
  if (result.error) throw result.error;
  return result;
}
function command(program, args) {
  const result = run(program, args);
  assert.ok(result.status !== null && !result.status, `${program}: ${result.stderr}`);
  return result.stdout;
}
let client;
let failure;
try {
  const actualRevision = command('git', ['rev-parse', 'HEAD']).trim();
  assert.equal(actualRevision, expectedRevision, 'the runner must identify the actual source revision');
  writeFileSync(join(output, 'source.patch'), command('git', ['diff', 'HEAD', '--binary']));
  report.binaryVersion = command(binary, ['--version']).trim();
  client = startRpc({
    binary, cwd: firstWorkspace,
    env: { ...process.env, HOME: home, JEDEN_SESSION_ROOT: join(output, 'sessions') },
    report,
  });
  for (const params of [{}, { cwd: null }, { cwd: '' }, { cwd: ' \t\n' }, { cwd: true }, { cwd: [] }]) {
    await assert.rejects(client.rpc('hillclimb/status', params),
      (error) => error.code === 'invalid_params');
  }
  const first = await client.rpc('hillclimb/status', { cwd: firstWorkspace });
  const second = await client.rpc('hillclimb/status', { cwd: secondWorkspace });
  assert.deepEqual(first.rounds, []);
  assert.deepEqual(second.rounds, []);
  assert.equal(first.accepted, first.rounds.length, 'an empty ledger has no accepted rounds');
  assert.equal(second.accepted, second.rounds.length, 'an empty ledger has no accepted rounds');
  assert.notEqual(first.ledger, second.ledger, 'distinct named workspaces have distinct ledgers');
  assert.equal(existsSync(first.ledger), false, 'a read does not create the first ledger');
  assert.equal(existsSync(second.ledger), false, 'a read does not create the second ledger');
  await assert.rejects(client.rpc('hillclimb/status', { cwd: join(output, 'missing') }),
    (error) => error.code === 'hillclimb_unreadable' && error.message.includes(join(output, 'missing')));
  const stillReadable = await client.rpc('hillclimb/status', { cwd: secondWorkspace });
  assert.deepEqual(stillReadable, second, 'a refused workspace read does not change another workspace');
  for (const ledger of [true, [], '', ' \t\n']) {
    await assert.rejects(client.rpc('hillclimb/status', { cwd: secondWorkspace, ledger }),
      (error) => error.code === 'invalid_params');
  }
  const selected = 'selected.jsonl';
  const selectedPath = join(secondWorkspace, selected);
  const overridden = await client.rpc('hillclimb/status', { cwd: secondWorkspace, ledger: selected });
  assert.equal(overridden.ledger, selectedPath, 'relative overrides belong to the named workspace');
  assert.deepEqual(overridden.rounds, []);
  assert.equal(existsSync(selectedPath), false, 'reading a selected ledger does not create it');
  const statusArgs = ['hillclimb', '--status', '--ledger', selected, '--cwd', secondWorkspace, '--json'];
  assert.deepEqual(JSON.parse(command(binary, statusArgs)), overridden,
    'the CLI reads the same selected ledger without write or command grants');
  const malformed = '{this is not a ledger round}\n';
  writeFileSync(selectedPath, malformed);
  await assert.rejects(client.rpc('hillclimb/status', { cwd: secondWorkspace, ledger: selected }),
    (error) => error.code === 'hillclimb_unreadable' && error.message.includes(selectedPath));
  const failedRead = run(binary, statusArgs);
  assert.ok(failedRead.status !== null && failedRead.status, 'the CLI must refuse the malformed ledger');
  assert.ok(failedRead.stderr.includes(selectedPath), 'the CLI identifies the ledger it could not read');
  assert.equal(readFileSync(selectedPath, 'utf8'), malformed, 'a refused read never rewrites the ledger');
  assert.deepEqual(await client.rpc('hillclimb/status', { cwd: firstWorkspace }), first,
    'a selected-ledger failure does not substitute or change another workspace ledger');
  await client.rpc('shutdown', {});
  report.verdict = 'passed';
} catch (error) {
  report.error = error.stack;
  failure = error;
  throw error;
} finally {
  let stopFailure;
  try {
    if (client) {
      const stopped = await client.end();
      assert.ok(stopped.status !== null && !stopped.status, stopped.stderr);
    }
  } catch (error) {
    report.stopFailure = error.stack;
    report.verdict = 'failed';
    stopFailure = error;
  } finally {
    report.finishedAt = new Date().toISOString();
    writeFileSync(join(output, 'report.json'), JSON.stringify(report, null, '\t'));
    console.log(join(output, 'report.json'));
  }
  if (stopFailure) {
    if (failure) throw new AggregateError([failure, stopFailure], 'The status journey and RPC cleanup both failed.');
    throw stopFailure;
  }
}
